// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! Tool 1: session — create/list/load/search/update_metadata/delete

use crate::daemon::coerce::{CoercionError, coerce_and_validate};
use crate::daemon::validate::{validate_session_action, validate_session_id};
use post_cortex_mcp::MCPToolResult;
use post_cortex_memory::ConversationMemorySystem;
use rmcp::{
    handler::server::wrapper::Parameters,
    model::{CallToolResult, Content, ErrorData as McpError},
};
use std::sync::Arc;
use uuid::Uuid;

use super::SessionRequest;
use super::mcp_result_to_call_result;

pub(super) async fn handle(
    memory_system: &Arc<ConversationMemorySystem>,
    params: Parameters<serde_json::Value>,
) -> Result<CallToolResult, McpError> {
    let req: SessionRequest = coerce_and_validate(params.0).map_err(|e| {
        e.with_parameter_path("action".to_string())
            .with_expected_type("one of: create, list, load, search, update_metadata, delete")
            .with_hint("Valid actions: create (name, description), list, load (session_id), search (query), update_metadata (session_id + name/description), delete (session_id)")
            .to_mcp_error()
    })?;

    validate_session_action(&req.action).map_err(|e| e.to_mcp_error())?;

    let require_session_id = |req: &SessionRequest, action: &str| -> Result<Uuid, McpError> {
        let raw = req.session_id.as_ref().ok_or_else(|| {
            CoercionError::new(
                "Missing required parameter",
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "session_id required"),
                None,
            )
            .with_parameter_path("session_id".to_string())
            .with_expected_type("UUID string (36 chars with hyphens)")
            .with_hint(&format!(
                "Action '{}' requires session_id (the session UUID)",
                action
            ))
            .to_mcp_error()
        })?;
        validate_session_id(raw).map_err(|e| e.to_mcp_error())
    };

    match req.action.to_lowercase().as_str() {
        "create" => {
            match memory_system
                .create_session(req.name.clone(), req.description.clone())
                .await
            {
                Ok(session_id) => Ok(CallToolResult::success(vec![Content::text(format!(
                    "Created session: {}{}{}",
                    session_id,
                    req.name
                        .as_ref()
                        .map(|n| format!(" (name: {})", n))
                        .unwrap_or_default(),
                    req.description
                        .as_ref()
                        .map(|d| format!(" - {}", d))
                        .unwrap_or_default(),
                ))])),
                Err(e) => Err(McpError::internal_error(e.to_string(), None)),
            }
        }
        "list" => match post_cortex_mcp::list_sessions().await {
            Ok(result) => Ok(CallToolResult::success(vec![Content::text(result.message)])),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        },
        "load" => {
            let session_id = require_session_id(&req, "load")?;
            let session_arc = memory_system
                .get_session(session_id)
                .await
                .map_err(|e| McpError::internal_error(format!("Session not found: {}", e), None))?;
            let session = session_arc.load();

            let info = serde_json::json!({
                "session_id": session_id.to_string(),
                "name": session.name().unwrap_or_default(),
                "description": session.description().unwrap_or_default(),
                "created_at_unix": session.created_at().timestamp(),
                "update_count": session.incremental_updates.len(),
            });

            Ok(mcp_result_to_call_result(MCPToolResult::success(
                format!("Loaded session {}", session_id),
                Some(info),
            )))
        }
        "search" => {
            let query = req.query.as_ref().ok_or_else(|| {
                CoercionError::new(
                    "Missing required parameter",
                    std::io::Error::new(std::io::ErrorKind::InvalidInput, "query required"),
                    None,
                )
                .with_parameter_path("query".to_string())
                .with_expected_type("search query string")
                .with_hint(
                    "Action 'search' requires a non-empty 'query' for name/description substring match",
                )
                .to_mcp_error()
            })?;

            if query.is_empty() {
                return Err(McpError::invalid_params(
                    "query cannot be empty".to_string(),
                    Some(serde_json::Value::String("query".to_string())),
                ));
            }

            let session_ids = memory_system
                .find_sessions_by_name_or_description(query)
                .await
                .map_err(|e| McpError::internal_error(e.to_string(), None))?;

            let mut entries = Vec::with_capacity(session_ids.len());
            for sid in &session_ids {
                if let Ok(session_arc) = memory_system.get_session(*sid).await {
                    let session = session_arc.load();
                    entries.push(serde_json::json!({
                        "session_id": sid.to_string(),
                        "name": session.name().unwrap_or_default(),
                        "description": session.description().unwrap_or_default(),
                        "created_at_unix": session.created_at().timestamp(),
                        "update_count": session.incremental_updates.len(),
                    }));
                }
            }

            Ok(mcp_result_to_call_result(MCPToolResult::success(
                format!("Found {} matching sessions", entries.len()),
                Some(serde_json::json!({ "query": query, "sessions": entries })),
            )))
        }
        "update_metadata" => {
            let session_id = require_session_id(&req, "update_metadata")?;
            if req.name.is_none() && req.description.is_none() {
                return Err(McpError::invalid_params(
                    "update_metadata requires at least one of 'name' or 'description'".to_string(),
                    None,
                ));
            }
            match memory_system
                .update_session_metadata(session_id, req.name.clone(), req.description.clone())
                .await
            {
                Ok(()) => Ok(CallToolResult::success(vec![Content::text(format!(
                    "Updated metadata for session {}",
                    session_id
                ))])),
                Err(e) => Err(McpError::internal_error(e.to_string(), None)),
            }
        }
        "delete" => {
            let session_id = require_session_id(&req, "delete")?;
            match memory_system.delete_session(session_id).await {
                Ok(true) => Ok(CallToolResult::success(vec![Content::text(format!(
                    "Deleted session {}",
                    session_id
                ))])),
                Ok(false) => Ok(CallToolResult::success(vec![Content::text(format!(
                    "Session {} was not present (no-op)",
                    session_id
                ))])),
                Err(e) => Err(McpError::internal_error(e.to_string(), None)),
            }
        }
        _ => Err(McpError::invalid_params(
            format!(
                "Invalid action '{}'. Use: create | list | load | search | update_metadata | delete",
                req.action
            ),
            None,
        )),
    }
}
