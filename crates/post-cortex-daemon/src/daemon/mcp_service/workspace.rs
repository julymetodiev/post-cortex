// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! Tool 6: manage_workspace — create/list/get/delete/add_session/remove_session,
//! and the project / user cards (get_card/update_card).

use crate::daemon::coerce::{CoercionError, coerce_and_validate};
use crate::daemon::validate::{
    validate_session_id, validate_session_role, validate_workspace_action, validate_workspace_id,
};
use post_cortex_core::core::card::CardScope;
use post_cortex_core::core::error::SystemError;
use post_cortex_core::services::{GetCardRequest, PostCortexService as _, UpdateCardRequest};
use post_cortex_mcp::MCPToolResult;
use post_cortex_memory::ConversationMemorySystem;
use rmcp::{
    handler::server::wrapper::Parameters,
    model::{CallToolResult, ErrorData as McpError},
};
use std::sync::Arc;

use super::ManageWorkspaceRequest;

pub(super) async fn handle(
    _memory_system: &Arc<ConversationMemorySystem>,
    params: Parameters<serde_json::Value>,
) -> Result<CallToolResult, McpError> {
    let req: ManageWorkspaceRequest = coerce_and_validate(params.0)
        .map_err(|e| {
            if e.message.contains("action") {
                e.clone()
                    .with_parameter_path("action".to_string())
                    .with_expected_type("one of: create, list, get, delete, add_session, remove_session")
                    .with_hint("Valid actions: create (workspace), list (all), get (workspace details), delete (workspace), add_session (to workspace), remove_session (from workspace)")
                    .to_mcp_error()
            } else {
                e.to_mcp_error()
            }
        })?;

    validate_workspace_action(&req.action).map_err(|e| e.to_mcp_error())?;

    match req.action.to_lowercase().as_str() {
        "create" => {
            let name = req.name.as_ref().ok_or_else(|| {
                CoercionError::new(
                    "Missing required parameter",
                    std::io::Error::new(std::io::ErrorKind::InvalidInput, "name required"),
                    None,
                )
                .with_parameter_path("name".to_string())
                .with_expected_type("workspace name string")
                .with_hint(
                    "For create action, provide 'name' and 'description' for the new workspace",
                )
                .to_mcp_error()
            })?;
            let description = req.description.as_ref().ok_or_else(|| {
                CoercionError::new(
                    "Missing required parameter",
                    std::io::Error::new(std::io::ErrorKind::InvalidInput, "description required"),
                    None,
                )
                .with_parameter_path("description".to_string())
                .with_expected_type("workspace description string")
                .with_hint(
                    "For create action, provide 'name' and 'description' for the new workspace",
                )
                .to_mcp_error()
            })?;

            match post_cortex_mcp::create_workspace(name.clone(), description.clone()).await {
                Ok(result) => Ok(super::mcp_result_to_call_result(result)),
                Err(e) => Err(McpError::internal_error(e.to_string(), None)),
            }
        }
        "list" => match post_cortex_mcp::list_workspaces().await {
            Ok(result) => Ok(super::mcp_result_to_call_result(result)),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        },
        "get" => {
            let workspace_id = req.workspace_id.as_ref().ok_or_else(|| {
                CoercionError::new(
                    "Missing required parameter",
                    std::io::Error::new(std::io::ErrorKind::InvalidInput, "workspace_id required"),
                    None,
                )
                .with_parameter_path("workspace_id".to_string())
                .with_expected_type("workspace UUID string")
                .with_hint("For get action, provide 'workspace_id' of the workspace to retrieve")
                .to_mcp_error()
            })?;

            let uuid = validate_workspace_id(workspace_id).map_err(|e| e.to_mcp_error())?;

            match post_cortex_mcp::get_workspace(uuid).await {
                Ok(result) => Ok(super::mcp_result_to_call_result(result)),
                Err(e) => Err(McpError::internal_error(e.to_string(), None)),
            }
        }
        "delete" => {
            let workspace_id = req.workspace_id.as_ref().ok_or_else(|| {
                CoercionError::new(
                    "Missing required parameter",
                    std::io::Error::new(std::io::ErrorKind::InvalidInput, "workspace_id required"),
                    None,
                )
                .with_parameter_path("workspace_id".to_string())
                .with_expected_type("workspace UUID string")
                .with_hint("For delete action, provide 'workspace_id' of the workspace to delete")
                .to_mcp_error()
            })?;

            let uuid = validate_workspace_id(workspace_id).map_err(|e| e.to_mcp_error())?;

            match post_cortex_mcp::delete_workspace(uuid).await {
                Ok(result) => Ok(super::mcp_result_to_call_result(result)),
                Err(e) => Err(McpError::internal_error(e.to_string(), None)),
            }
        }
        "add_session" => {
            let workspace_id = req.workspace_id.as_ref().ok_or_else(|| {
                CoercionError::new(
                    "Missing required parameter",
                    std::io::Error::new(std::io::ErrorKind::InvalidInput, "workspace_id required"),
                    None,
                )
                .with_parameter_path("workspace_id".to_string())
                .with_expected_type("workspace UUID string")
                .with_hint("For add_session action, provide 'workspace_id' and 'session_id'")
                .to_mcp_error()
            })?;
            let session_id = req.session_id.as_ref().ok_or_else(|| {
                CoercionError::new(
                    "Missing required parameter",
                    std::io::Error::new(std::io::ErrorKind::InvalidInput, "session_id required"),
                    None,
                )
                .with_parameter_path("session_id".to_string())
                .with_expected_type("session UUID string")
                .with_hint("For add_session action, provide 'workspace_id' and 'session_id'")
                .to_mcp_error()
            })?;
            let role = req.role.clone().unwrap_or_else(|| "related".to_string());

            let ws_uuid = validate_workspace_id(workspace_id).map_err(|e| e.to_mcp_error())?;
            let sess_uuid = validate_session_id(session_id).map_err(|e| e.to_mcp_error())?;

            validate_session_role(&role).map_err(|e| e.to_mcp_error())?;

            match post_cortex_mcp::add_session_to_workspace(ws_uuid, sess_uuid, role).await {
                Ok(result) => Ok(super::mcp_result_to_call_result(result)),
                Err(e) => Err(McpError::internal_error(e.to_string(), None)),
            }
        }
        "remove_session" => {
            let workspace_id = req.workspace_id.as_ref().ok_or_else(|| {
                CoercionError::new(
                    "Missing required parameter",
                    std::io::Error::new(std::io::ErrorKind::InvalidInput, "workspace_id required"),
                    None,
                )
                .with_parameter_path("workspace_id".to_string())
                .with_expected_type("workspace UUID string")
                .with_hint("For remove_session action, provide 'workspace_id' and 'session_id'")
                .to_mcp_error()
            })?;
            let session_id = req.session_id.as_ref().ok_or_else(|| {
                CoercionError::new(
                    "Missing required parameter",
                    std::io::Error::new(std::io::ErrorKind::InvalidInput, "session_id required"),
                    None,
                )
                .with_parameter_path("session_id".to_string())
                .with_expected_type("session UUID string")
                .with_hint("For remove_session action, provide 'workspace_id' and 'session_id'")
                .to_mcp_error()
            })?;

            let ws_uuid = validate_workspace_id(workspace_id).map_err(|e| e.to_mcp_error())?;
            let sess_uuid = validate_session_id(session_id).map_err(|e| e.to_mcp_error())?;

            match post_cortex_mcp::remove_session_from_workspace(ws_uuid, sess_uuid).await {
                Ok(result) => Ok(super::mcp_result_to_call_result(result)),
                Err(e) => Err(McpError::internal_error(e.to_string(), None)),
            }
        }
        "get_card" => {
            let scope = card_scope(&req)?;
            let response = card_service()
                .await?
                .get_card(GetCardRequest {
                    scope,
                    history: req.history.unwrap_or(0) as usize,
                })
                .await
                .map_err(system_error)?;
            let message = match &response.card {
                Some(card) => format!(
                    "Card {} version {} ({} bytes)",
                    scope.key(),
                    card.version,
                    card.text.len()
                ),
                None => format!(
                    "No card for {} yet (create one with expected_version 0)",
                    scope.key()
                ),
            };
            Ok(super::mcp_result_to_call_result(MCPToolResult::success(
                message,
                Some(serde_json::to_value(&response).unwrap_or_default()),
            )))
        }
        "update_card" => {
            let scope = card_scope(&req)?;
            let content = req.content.clone().ok_or_else(|| {
                McpError::invalid_params(
                    "update_card needs content (the whole new card text)".to_string(),
                    Some(serde_json::json!({"parameter": "content"})),
                )
            })?;
            let expected_version = req.expected_version.ok_or_else(|| {
                McpError::invalid_params(
                    "update_card needs expected_version: the version you read with get_card (0 for a new card)"
                        .to_string(),
                    Some(serde_json::json!({"parameter": "expected_version"})),
                )
            })?;
            let response = card_service()
                .await?
                .update_card(UpdateCardRequest {
                    scope,
                    text: content,
                    expected_version,
                    author: None,
                })
                .await
                .map_err(system_error)?;
            let message = match (&response.written, &response.card) {
                (true, Some(card)) => {
                    format!("Card {} saved as version {}", scope.key(), card.version)
                }
                (_, Some(card)) => format!(
                    "Not saved: the card is at version {} now, not {}. Merge your change into the text below and retry with expected_version {}",
                    card.version, expected_version, card.version
                ),
                (_, None) => {
                    "Not saved: there is no card yet; retry with expected_version 0".to_string()
                }
            };
            Ok(super::mcp_result_to_call_result(MCPToolResult::success(
                message,
                Some(serde_json::to_value(&response).unwrap_or_default()),
            )))
        }
        _ => Err(McpError::invalid_params(
            format!(
                "Invalid action '{}'. Use: create | list | get | delete | add_session | remove_session | get_card | update_card",
                req.action
            ),
            None,
        )),
    }
}

/// The card `req` names: the user card, or the project card of its
/// `workspace_id`.
fn card_scope(req: &ManageWorkspaceRequest) -> Result<CardScope, McpError> {
    match req.card.as_deref().map(str::to_lowercase).as_deref() {
        Some("user") => Ok(CardScope::User),
        None | Some("workspace") => {
            let workspace_id = req.workspace_id.as_ref().ok_or_else(|| {
                McpError::invalid_params(
                    "workspace_id is required for a project card (or set card: \"user\")"
                        .to_string(),
                    Some(serde_json::json!({"parameter": "workspace_id"})),
                )
            })?;
            Ok(CardScope::Workspace(
                validate_workspace_id(workspace_id).map_err(|e| e.to_mcp_error())?,
            ))
        }
        Some(other) => Err(McpError::invalid_params(
            format!("Invalid card '{other}': expected 'workspace' or 'user'"),
            Some(serde_json::json!({"parameter": "card"})),
        )),
    }
}

async fn card_service() -> Result<Arc<post_cortex_memory::services::MemoryServiceImpl>, McpError> {
    post_cortex_mcp::get_service()
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))
}

fn system_error(e: SystemError) -> McpError {
    match e {
        SystemError::InvalidArgument(_) | SystemError::WorkspaceNotFound(_) => {
            McpError::invalid_params(e.to_string(), None)
        }
        e => McpError::internal_error(e.to_string(), None),
    }
}
