// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! Tool 2: update_conversation_context — single or bulk updates with dry-run.

use post_cortex_memory::ConversationMemorySystem;
use crate::daemon::coerce::{CoercionError, coerce_and_validate};
use crate::daemon::validate::{validate_interaction_type, validate_session_id};
use rmcp::{
    handler::server::wrapper::Parameters,
    model::{CallToolResult, Content, ErrorData as McpError},
};
use std::sync::Arc;

use super::UpdateConversationContextRequest;
use super::check_session_exists_for_dry_run;

pub(super) async fn handle(
    memory_system: &Arc<ConversationMemorySystem>,
    params: Parameters<serde_json::Value>,
) -> Result<CallToolResult, McpError> {
    let req: UpdateConversationContextRequest = coerce_and_validate(params.0)
        .map_err(|e| {
            // Enhance error with parameter-specific hints
            if e.message.contains("session_id") {
                e.clone()
                    .with_parameter_path("session_id".to_string())
                    .with_expected_type("UUID string (36 chars with hyphens)")
                    .with_hint("Create a session first using the 'session' tool with action='create', then use the returned UUID")
                    .to_mcp_error()
            } else if e.message.contains("interaction_type") {
                e.clone()
                    .with_parameter_path("interaction_type".to_string())
                    .with_expected_type("one of: qa, decision_made, problem_solved, code_change, requirement_added, concept_defined")
                    .with_hint("Valid interaction types: qa, decision_made, problem_solved, code_change, requirement_added, concept_defined")
                    .to_mcp_error()
            } else if e.message.contains("content") {
                e.clone()
                    .with_parameter_path("content".to_string())
                    .with_expected_type("object with string key-value pairs")
                    .with_hint("Content must be a map of string keys to string values. For complex data, stringify as JSON first.")
                    .to_mcp_error()
            } else {
                e.to_mcp_error()
            }
        })?;

    let uuid = validate_session_id(&req.session_id).map_err(|e| e.to_mcp_error())?;

    // Handle dry_run mode for bulk updates
    if let Some(ref updates) = req.updates {
        if req.dry_run.unwrap_or(false) {
            for (i, update) in updates.iter().enumerate() {
                validate_interaction_type(&update.interaction_type).map_err(|e| {
                    e.clone()
                        .with_parameter_path(format!("updates[{}].interaction_type", i))
                        .to_mcp_error()
                })?;
            }

            let preview: Vec<String> = updates
                .iter()
                .enumerate()
                .map(|(i, u)| {
                    format!(
                        "  [{}] {} - fields: {}",
                        i + 1,
                        u.interaction_type,
                        u.content.keys().cloned().collect::<Vec<_>>().join(", ")
                    )
                })
                .collect();

            check_session_exists_for_dry_run(memory_system, uuid).await?;

            return Ok(CallToolResult::success(vec![Content::text(format!(
                "✅ Dry run - Bulk update validation successful\n\
                 Session ID: {}\n\
                 Total updates: {}\n\n\
                 Preview:\n{}\n\n\
                 No changes were made. Set dry_run to false or omit it to actually save these updates.",
                uuid,
                updates.len(),
                preview.join("\n")
            ))]));
        }

        let items: Vec<post_cortex_mcp::ContextUpdateItem> = updates
            .iter()
            .map(|u| post_cortex_mcp::ContextUpdateItem {
                interaction_type: u.interaction_type.clone(),
                content: u.content.clone(),
                entities: u.entities.clone(),
                relations: u.relations.clone(),
                code_reference: u
                    .code_reference
                    .as_ref()
                    .and_then(|v| serde_json::from_value(v.clone()).ok()),
            })
            .collect();

        match post_cortex_mcp::bulk_update_conversation_context(items, uuid).await {
            Ok(result) => Ok(CallToolResult::success(vec![Content::text(result.message)])),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    } else {
        // Single update mode
        let interaction_type = req.interaction_type.as_ref()
            .ok_or_else(|| {
                CoercionError::new(
                    "Missing required parameter",
                    std::io::Error::new(std::io::ErrorKind::InvalidInput, "interaction_type required"),
                    None,
                )
                .with_parameter_path("interaction_type".to_string())
                .with_expected_type("one of: qa, decision_made, problem_solved, code_change, requirement_added, concept_defined")
                .with_hint("For single update, provide 'interaction_type' and 'content'. For bulk updates, use 'updates' array instead.")
                .to_mcp_error()
            })?;

        validate_interaction_type(interaction_type).map_err(|e| e.to_mcp_error())?;
        let content = req.content.as_ref()
            .ok_or_else(|| {
                CoercionError::new(
                    "Missing required parameter",
                    std::io::Error::new(std::io::ErrorKind::InvalidInput, "content required"),
                    None,
                )
                .with_parameter_path("content".to_string())
                .with_expected_type("object with string key-value pairs")
                .with_hint("For single update, provide 'content' and 'interaction_type'. For bulk updates, use 'updates' array instead.")
                .to_mcp_error()
            })?;

        let code_ref = req
            .code_reference
            .as_ref()
            .and_then(|v| serde_json::from_value(v.clone()).ok());

        // Handle dry_run mode for single updates
        if req.dry_run.unwrap_or(false) {
            check_session_exists_for_dry_run(memory_system, uuid).await?;

            return Ok(CallToolResult::success(vec![Content::text(format!(
                "✅ Dry run - Request validation successful\n\
                 Session ID: {}\n\
                 Interaction Type: {}\n\
                 Content fields: {}\n\
                 Code Reference: {}\n\n\
                 No changes were made. Set dry_run to false or omit it to actually save this update.",
                uuid,
                interaction_type,
                content.keys().cloned().collect::<Vec<_>>().join(", "),
                code_ref.as_ref().map(|_| "provided").unwrap_or("none")
            ))]));
        }

        match post_cortex_mcp::update_conversation_context(
            interaction_type.clone(),
            content.clone(),
            req.entities.clone(),
            req.relations.clone(),
            code_ref,
            uuid,
        )
        .await
        {
            Ok(result) => Ok(CallToolResult::success(vec![Content::text(result.message)])),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }
}
