// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! Tool 3: semantic_search — session / workspace / global scope.

use crate::ConversationMemorySystem;
use crate::daemon::coerce::{CoercionError, coerce_and_validate};
use crate::daemon::validate::{
    validate_limits, validate_recency_bias, validate_scope, validate_session_id,
};
use crate::tools::mcp::{MCPToolResult, get_memory_system};
use rmcp::{
    handler::server::wrapper::Parameters,
    model::{CallToolResult, ErrorData as McpError},
};
use std::sync::Arc;
use uuid::Uuid;

use super::SemanticSearchRequest;
use super::{mcp_result_to_call_result, parse_date_range};

pub(super) async fn handle(
    _memory_system: &Arc<ConversationMemorySystem>,
    params: Parameters<serde_json::Value>,
) -> Result<CallToolResult, McpError> {
    let req: SemanticSearchRequest = coerce_and_validate(params.0)
        .map_err(|e| {
            if e.message.contains("scope") {
                e.clone()
                    .with_parameter_path("scope".to_string())
                    .with_expected_type("one of: session, workspace, global")
                    .with_hint("Valid scopes: 'session' (requires scope_id), 'workspace' (requires scope_id), 'global' (default)")
                    .to_mcp_error()
            } else if e.message.contains("scope_id") {
                e.clone()
                    .with_parameter_path("scope_id".to_string())
                    .with_expected_type("UUID string (required when scope is 'session' or 'workspace')")
                    .with_hint("When scope is 'session' or 'workspace', provide the session/workspace UUID as scope_id")
                    .to_mcp_error()
            } else if e.message.contains("query") {
                e.clone()
                    .with_parameter_path("query".to_string())
                    .with_expected_type("search query string")
                    .with_hint("Provide a text query to search for in the conversation history")
                    .to_mcp_error()
            } else {
                e.to_mcp_error()
            }
        })?;

    let scope = req.scope.as_deref().unwrap_or("global");

    validate_scope(scope).map_err(|e| e.to_mcp_error())?;

    // Validate limit parameter (default: 10, max: 1000)
    let validated_limit = validate_limits(req.limit, 10, 1000)
        .map_err(|e| e.with_parameter_path("limit".to_string()).to_mcp_error())?;

    match scope.to_lowercase().as_str() {
        "session" => {
            let session_id = req.scope_id.as_ref().ok_or_else(|| {
                CoercionError::new(
                    "Missing required parameter",
                    std::io::Error::new(std::io::ErrorKind::InvalidInput, "scope_id required"),
                    None,
                )
                .with_parameter_path("scope_id".to_string())
                .with_expected_type("UUID string")
                .with_hint("When scope is 'session', you must provide scope_id (the session UUID)")
                .to_mcp_error()
            })?;

            let uuid = validate_session_id(session_id).map_err(|e| e.to_mcp_error())?;

            let validated_recency_bias =
                validate_recency_bias(req.recency_bias).map_err(|e| e.to_mcp_error())?;

            let search_results = crate::tools::mcp::semantic_search_session(
                uuid,
                req.query.clone(),
                Some(validated_limit),
                req.date_from.clone(),
                req.date_to.clone(),
                req.interaction_type.clone(),
                validated_recency_bias,
            )
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

            Ok(mcp_result_to_call_result(search_results))
        }
        "workspace" => {
            let ws_id = req.scope_id.as_ref().ok_or_else(|| {
                CoercionError::new(
                    "Missing required parameter",
                    std::io::Error::new(std::io::ErrorKind::InvalidInput, "scope_id required"),
                    None,
                )
                .with_parameter_path("scope_id".to_string())
                .with_expected_type("UUID string")
                .with_hint(
                    "When scope is 'workspace', you must provide scope_id (the workspace UUID)",
                )
                .to_mcp_error()
            })?;

            let ws_uuid = Uuid::parse_str(ws_id).map_err(|_| {
                CoercionError::new(
                    "Invalid UUID",
                    std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid UUID format"),
                    None,
                )
                .with_parameter_path("scope_id".to_string())
                .with_expected_type("Valid UUID string")
                .to_mcp_error()
            })?;

            let system = get_memory_system().await.map_err(|e| {
                McpError::internal_error(format!("Failed to get memory system: {}", e), None)
            })?;

            let workspace = system
                .workspace_manager
                .get_workspace(&ws_uuid)
                .ok_or_else(|| {
                    McpError::internal_error(format!("Workspace {} not found", ws_uuid), None)
                })?;

            let session_ids: Vec<Uuid> = workspace
                .get_all_sessions()
                .into_iter()
                .map(|(id, _)| id)
                .collect();

            // semantic_search_multisession treats an empty session_ids slice as
            // "no filter" and falls back to a global search. For workspace scope
            // that's wrong — an empty workspace must return zero results.
            if session_ids.is_empty() {
                return Ok(mcp_result_to_call_result(MCPToolResult::success(
                    format!("Found 0 results (workspace {} has no sessions)", ws_uuid),
                    Some(serde_json::json!({
                        "results": [],
                        "workspace_id": ws_uuid.to_string(),
                        "note": "workspace has no sessions",
                    })),
                )));
            }

            let date_range = parse_date_range(req.date_from.clone(), req.date_to.clone())?;

            let validated_recency_bias =
                validate_recency_bias(req.recency_bias).map_err(|e| e.to_mcp_error())?;

            let results = system
                .semantic_search_multisession(
                    &session_ids,
                    &req.query,
                    Some(validated_limit),
                    date_range,
                    validated_recency_bias,
                )
                .await
                .map_err(|e| McpError::internal_error(e, None))?;

            let formatted = crate::daemon::format_helpers::format_search_results(&results);

            let search_results = MCPToolResult::success(
                format!("Found {} results", results.len()),
                Some(serde_json::json!({ "results": formatted })),
            );

            Ok(mcp_result_to_call_result(search_results))
        }
        "global" | _ => {
            let validated_recency_bias =
                validate_recency_bias(req.recency_bias).map_err(|e| e.to_mcp_error())?;

            let search_results = crate::tools::mcp::semantic_search_global(
                req.query.clone(),
                Some(validated_limit),
                req.date_from.clone(),
                req.date_to.clone(),
                req.interaction_type.clone(),
                validated_recency_bias,
            )
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

            Ok(mcp_result_to_call_result(search_results))
        }
    }
}
