// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! Tool 8: manage_entity — delete entity / delete_update.

use crate::ConversationMemorySystem;
use crate::daemon::coerce::{CoercionError, coerce_and_validate};
use crate::daemon::validate::validate_session_id;
use crate::tools::mcp::MCPToolResult;
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
                .with_expected_type("one of: delete, delete_update")
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
        "delete_update" => {
            let entry_raw = req.entry_id.as_ref().ok_or_else(|| {
                McpError::invalid_params(
                    "action 'delete_update' requires entry_id".to_string(),
                    Some(serde_json::Value::String("entry_id".to_string())),
                )
            })?;
            let entry_id = Uuid::parse_str(entry_raw).map_err(|_| {
                CoercionError::new(
                    "Invalid UUID",
                    std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid UUID"),
                    Some(serde_json::Value::String(entry_raw.clone())),
                )
                .with_parameter_path("entry_id".to_string())
                .with_expected_type("UUID string (36 chars with hyphens)")
                .with_hint("entry_id is the ContextUpdate id (e.g. the entry_id returned by assemble_context items)")
                .to_mcp_error()
            })?;

            match memory_system.delete_update(session_id, entry_id).await {
                Ok(existed) => Ok(mcp_result_to_call_result(MCPToolResult::success(
                    if existed {
                        format!("Deleted update {} from session {}", entry_id, session_id)
                    } else {
                        format!(
                            "Update {} not present in session {} (no-op)",
                            entry_id, session_id
                        )
                    },
                    Some(serde_json::json!({
                        "existed": existed,
                        "entry_id": entry_id.to_string(),
                        "session_id": session_id.to_string(),
                    })),
                ))),
                Err(e) => Err(McpError::internal_error(e.to_string(), None)),
            }
        }
        _ => Err(McpError::invalid_params(
            format!("Invalid action '{}'. Use: delete | delete_update", req.action),
            None,
        )),
    }
}
