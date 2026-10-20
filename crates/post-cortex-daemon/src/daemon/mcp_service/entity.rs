// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! Tool 8: manage_entity — delete / merge entities, delete / forget / purge records.

use crate::daemon::coerce::{CoercionError, coerce_and_validate};
use crate::daemon::validate::validate_session_id;
use post_cortex_mcp::MCPToolResult;
use post_cortex_memory::ConversationMemorySystem;
use rmcp::{
    handler::server::wrapper::Parameters,
    model::{CallToolResult, ErrorData as McpError},
};
use std::sync::Arc;
use uuid::Uuid;

use super::ManageEntityRequest;
use super::mcp_result_to_call_result;

pub(super) async fn handle(
    memory_system: &Arc<ConversationMemorySystem>,
    params: Parameters<serde_json::Value>,
) -> Result<CallToolResult, McpError> {
    let req: ManageEntityRequest = coerce_and_validate(params.0).map_err(|e| {
        if e.message.contains("action") {
            e.clone()
                .with_parameter_path("action".to_string())
                .with_expected_type("one of: delete, merge, delete_update, forget, purge")
                .with_hint(
                    "Use 'delete' with entity_name for entities, or 'delete_update' with entry_id for context rows",
                )
                .to_mcp_error()
        } else if e.message.contains("session_id") {
            e.clone()
                .with_parameter_path("session_id".to_string())
                .with_expected_type("UUID string (36 chars with hyphens)")
                .to_mcp_error()
        } else {
            e.to_mcp_error()
        }
    })?;

    let session_id = validate_session_id(&req.session_id).map_err(|e| e.to_mcp_error())?;

    match req.action.to_lowercase().as_str() {
        "delete" => {
            let entity_name = req
                .entity_name
                .as_ref()
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    McpError::invalid_params(
                        "action 'delete' requires a non-empty entity_name".to_string(),
                        Some(serde_json::Value::String("entity_name".to_string())),
                    )
                })?;

            match memory_system.delete_entity(session_id, entity_name).await {
                Ok(existed) => Ok(mcp_result_to_call_result(MCPToolResult::success(
                    if existed {
                        format!(
                            "Deleted entity '{}' from session {}",
                            entity_name, session_id
                        )
                    } else {
                        format!(
                            "Entity '{}' not present in session {} (no-op)",
                            entity_name, session_id
                        )
                    },
                    Some(serde_json::json!({
                        "existed": existed,
                        "entity_name": entity_name,
                        "session_id": session_id.to_string(),
                    })),
                ))),
                Err(e) => Err(McpError::internal_error(e.to_string(), None)),
            }
        }
        "merge" => {
            let name = |v: &Option<String>, field: &str| {
                v.as_ref()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| {
                        McpError::invalid_params(
                            format!("action 'merge' requires a non-empty {field}"),
                            Some(serde_json::Value::String(field.to_string())),
                        )
                    })
            };
            let alias = name(&req.entity_name, "entity_name")?;
            let into = name(&req.into, "into")?;
            let merged = memory_system
                .merge_entities(session_id, &alias, &into)
                .await
                .map_err(|e| McpError::internal_error(e, None))?;
            Ok(mcp_result_to_call_result(MCPToolResult::success(
                if merged {
                    format!("Merged entity '{alias}' into '{into}' in session {session_id}")
                } else {
                    format!(
                        "Nothing merged: '{alias}' or '{into}' is not in session {session_id}, or they are one entity"
                    )
                },
                Some(serde_json::json!({
                    "merged": merged,
                    "entity_name": alias,
                    "into": into,
                    "session_id": session_id.to_string(),
                })),
            )))
        }
        action @ ("delete_update" | "forget" | "purge") => {
            let entry_id = entry_id(&req, action)?;
            let result = match action {
                "delete_update" => memory_system.delete_update(session_id, entry_id).await,
                "forget" => memory_system.forget_update(session_id, entry_id).await,
                _ => memory_system.purge_update(session_id, entry_id).await,
            };
            let done = result.map_err(|e| McpError::internal_error(e, None))?;
            let verb = match action {
                "delete_update" => "Deleted",
                "forget" => "Retired",
                _ => "Purged",
            };
            Ok(mcp_result_to_call_result(MCPToolResult::success(
                if done {
                    format!("{verb} update {entry_id} in session {session_id}")
                } else if action == "forget" {
                    format!(
                        "Update {entry_id} not in session {session_id} or already retired (no-op)"
                    )
                } else {
                    format!("Update {entry_id} not present in session {session_id} (no-op)")
                },
                Some(serde_json::json!({
                    "existed": done,
                    "entry_id": entry_id.to_string(),
                    "session_id": session_id.to_string(),
                })),
            )))
        }
        _ => Err(McpError::invalid_params(
            format!(
                "Invalid action '{}'. Use: delete | merge | delete_update | forget | purge",
                req.action
            ),
            None,
        )),
    }
}

/// The `entry_id` an action on a record needs.
fn entry_id(req: &ManageEntityRequest, action: &str) -> Result<Uuid, McpError> {
    let entry_raw = req.entry_id.as_ref().ok_or_else(|| {
        McpError::invalid_params(
            format!("action '{action}' requires entry_id"),
            Some(serde_json::Value::String("entry_id".to_string())),
        )
    })?;
    Uuid::parse_str(entry_raw).map_err(|_| {
        CoercionError::new(
            "Invalid UUID",
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid UUID"),
            Some(serde_json::Value::String(entry_raw.clone())),
        )
        .with_parameter_path("entry_id".to_string())
        .with_expected_type("UUID string (36 chars with hyphens)")
        .with_hint("entry_id is the ContextUpdate id (e.g. the entry_id returned by assemble_context items)")
        .to_mcp_error()
    })
}
