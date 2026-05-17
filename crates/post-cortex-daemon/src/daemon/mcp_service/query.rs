// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! Tool 5: query_conversation_context — typed dispatch over many query kinds.

use crate::daemon::coerce::coerce_and_validate;
use crate::daemon::validate::validate_session_id;
use post_cortex_memory::ConversationMemorySystem;
use rmcp::{
    handler::server::wrapper::Parameters,
    model::{CallToolResult, Content, ErrorData as McpError},
};
use std::sync::Arc;

use super::QueryConversationContextRequest;
use super::mcp_result_to_call_result;

pub(super) async fn handle(
    _memory_system: &Arc<ConversationMemorySystem>,
    params: Parameters<serde_json::Value>,
) -> Result<CallToolResult, McpError> {
    let req: QueryConversationContextRequest = coerce_and_validate(params.0)
        .map_err(|e| {
            if e.message.contains("session_id") {
                e.clone()
                    .with_parameter_path("session_id".to_string())
                    .with_expected_type("UUID string (36 chars with hyphens)")
                    .with_hint("Provide the session UUID you want to query")
                    .to_mcp_error()
            } else if e.message.contains("query_type") {
                e.clone()
                    .with_parameter_path("query_type".to_string())
                    .with_expected_type("query type string (e.g., entity_importance, entity_network, find_related_entities)")
                    .with_hint("Specify the type of query: entity_importance, entity_network, find_related_entities, get_entity_context, search_updates")
                    .to_mcp_error()
            } else if e.message.contains("parameters") {
                e.clone()
                    .with_parameter_path("parameters".to_string())
                    .with_expected_type("object with string key-value pairs")
                    .with_hint("Query parameters vary by query_type. Provide as key-value pairs.")
                    .to_mcp_error()
            } else {
                e.to_mcp_error()
            }
        })?;

    let uuid = validate_session_id(&req.session_id).map_err(|e| e.to_mcp_error())?;

    match req.query_type.to_lowercase().as_str() {
        "entity_importance" => {
            let limit = req
                .parameters
                .get("limit")
                .and_then(|s: &String| s.parse().ok());
            let min_importance = req
                .parameters
                .get("min_importance")
                .and_then(|s: &String| s.parse().ok());

            match post_cortex_mcp::get_entity_importance_analysis(
                req.session_id.clone(),
                limit,
                min_importance,
            )
            .await
            {
                Ok(result) => Ok(mcp_result_to_call_result(result)),
                Err(e) => Err(McpError::internal_error(e.to_string(), None)),
            }
        }
        "entity_network" => {
            let center_entity = req.parameters.get("center_entity").cloned();
            let max_entities = req
                .parameters
                .get("max_entities")
                .and_then(|s: &String| s.parse().ok());
            let max_relationships = req
                .parameters
                .get("max_relationships")
                .and_then(|s: &String| s.parse().ok());

            match post_cortex_mcp::get_entity_network_view(
                req.session_id.clone(),
                center_entity,
                max_entities,
                max_relationships,
            )
            .await
            {
                Ok(result) => Ok(mcp_result_to_call_result(result)),
                Err(e) => Err(McpError::internal_error(e.to_string(), None)),
            }
        }
        "find_related_content" => {
            #[cfg(feature = "embeddings")]
            {
                let topic = req
                    .parameters
                    .get("topic")
                    .cloned()
                    .or_else(|| req.parameters.get("query").cloned())
                    .unwrap_or_default();
                if topic.is_empty() {
                    return Err(McpError::invalid_params(
                        "find_related_content requires a non-empty 'topic' parameter".to_string(),
                        Some(serde_json::Value::String("topic".to_string())),
                    ));
                }
                let limit = req
                    .parameters
                    .get("max_results")
                    .or_else(|| req.parameters.get("limit"))
                    .and_then(|s: &String| s.parse().ok());

                match post_cortex_mcp::find_related_content(uuid, topic, limit).await {
                    Ok(result) => Ok(mcp_result_to_call_result(result)),
                    Err(e) => Err(McpError::internal_error(e.to_string(), None)),
                }
            }
            #[cfg(not(feature = "embeddings"))]
            {
                let _ = uuid;
                Err(McpError::internal_error(
                    "find_related_content requires the 'embeddings' feature".to_string(),
                    None,
                ))
            }
        }
        "key_decisions" => match post_cortex_mcp::get_key_decisions(req.session_id.clone()).await {
            Ok(result) => Ok(mcp_result_to_call_result(result)),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        },
        "key_insights" => {
            let limit = req
                .parameters
                .get("limit")
                .and_then(|s: &String| s.parse().ok());
            match post_cortex_mcp::get_key_insights(req.session_id.clone(), limit).await {
                Ok(result) => Ok(mcp_result_to_call_result(result)),
                Err(e) => Err(McpError::internal_error(e.to_string(), None)),
            }
        }
        "session_statistics" => {
            match post_cortex_mcp::get_session_statistics(req.session_id.clone()).await {
                Ok(result) => Ok(mcp_result_to_call_result(result)),
                Err(e) => Err(McpError::internal_error(e.to_string(), None)),
            }
        }
        _ => {
            // Default: use the original query_conversation_context
            match post_cortex_mcp::query_conversation_context(
                req.query_type.clone(),
                req.parameters.clone(),
                uuid,
            )
            .await
            {
                Ok(result) => {
                    let mut contents = vec![Content::text(result.message)];
                    if let Some(data) = result.data {
                        contents.push(Content::text(format!(
                            "\n\nResults:\n{}",
                            serde_json::to_string_pretty(&data).unwrap_or_default()
                        )));
                    }
                    Ok(CallToolResult::success(contents))
                }
                Err(e) => Err(McpError::internal_error(e.to_string(), None)),
            }
        }
    }
}
