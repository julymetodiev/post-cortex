// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! Tool 7: assemble_context — graph-aware retrieval blending semantic search,
//! entity-graph traversal and impact analysis.

use post_cortex_memory::ConversationMemorySystem;
use post_cortex_memory::context_assembly;
use crate::daemon::coerce::{CoercionError, coerce_and_validate};
use crate::daemon::validate::validate_session_id;
use post_cortex_core::graph::entity_graph::SimpleEntityGraph;
use post_cortex_mcp::MCPToolResult;
use rmcp::{
    handler::server::wrapper::Parameters,
    model::{CallToolResult, ErrorData as McpError},
};
use std::sync::Arc;
use uuid::Uuid;

use super::AssembleContextRequest;
use super::mcp_result_to_call_result;

pub(super) async fn handle(
    memory_system: &Arc<ConversationMemorySystem>,
    params: Parameters<serde_json::Value>,
) -> Result<CallToolResult, McpError> {
    let req: AssembleContextRequest = coerce_and_validate(params.0).map_err(|e| {
        if e.message.contains("query") {
            e.clone()
                .with_parameter_path("query".to_string())
                .with_expected_type("non-empty query/topic string")
                .with_hint("Provide the question or topic to assemble context for")
                .to_mcp_error()
        } else {
            e.to_mcp_error()
        }
    })?;

    if req.query.trim().is_empty() {
        return Err(McpError::invalid_params(
            "query cannot be empty".to_string(),
            Some(serde_json::Value::String("query".to_string())),
        ));
    }

    if req.session_id.is_none() && req.workspace_id.is_none() {
        return Err(McpError::invalid_params(
            "either session_id or workspace_id is required".to_string(),
            None,
        ));
    }

    let token_budget = req
        .token_budget
        .filter(|b| *b > 0)
        .map(|b| b as usize)
        .unwrap_or(4000);

    let (updates, graph) = if let Some(ws_raw) =
        req.workspace_id.as_ref().filter(|s| !s.is_empty())
    {
        let ws_uuid = Uuid::parse_str(ws_raw).map_err(|_| {
            CoercionError::new(
                "Invalid UUID",
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid UUID format"),
                Some(serde_json::Value::String(ws_raw.clone())),
            )
            .with_parameter_path("workspace_id".to_string())
            .with_expected_type("Valid UUID string")
            .to_mcp_error()
        })?;

        let workspace = memory_system
            .workspace_manager
            .get_workspace(&ws_uuid)
            .ok_or_else(|| {
                McpError::invalid_params(
                    format!("Workspace {} not found", ws_uuid),
                    Some(serde_json::Value::String("workspace_id".to_string())),
                )
            })?;

        let mut all_updates = Vec::new();
        let mut merged_graph = SimpleEntityGraph::new();

        for (ws_session_id, _role) in workspace.get_all_sessions() {
            if let Ok(session_arc) = memory_system.get_session(ws_session_id).await {
                let session = session_arc.load();
                all_updates.extend(session.hot_context.iter().iter().cloned());
                all_updates.extend(session.warm_context.iter().map(|c| c.update.clone()));
                merged_graph.merge_from(&session.entity_graph);
            }
        }

        (all_updates, merged_graph)
    } else {
        let sid_raw = req.session_id.as_ref().unwrap();
        let session_id = validate_session_id(sid_raw).map_err(|e| e.to_mcp_error())?;

        let session_arc = memory_system
            .get_session(session_id)
            .await
            .map_err(|e| McpError::internal_error(format!("Session not found: {}", e), None))?;
        let session = session_arc.load();
        let updates: Vec<_> = session
            .hot_context
            .iter()
            .iter()
            .chain(session.warm_context.iter().map(|c| &c.update))
            .cloned()
            .collect();
        (updates, (*session.entity_graph).clone())
    };

    let assembled =
        context_assembly::assemble_context(&req.query, &graph, &updates, token_budget);
    let formatted_text = context_assembly::format_for_llm(&assembled);

    let payload = serde_json::json!({
        "query": req.query,
        "token_budget": token_budget,
        "total_tokens": assembled.total_tokens,
        "items": assembled.items,
        "entity_context": assembled.entity_context,
        "impact": assembled.impact,
        "formatted_text": formatted_text,
    });

    Ok(mcp_result_to_call_result(MCPToolResult::success(
        format!(
            "Assembled {} items ({} tokens) for query: {}",
            assembled.items.len(),
            assembled.total_tokens,
            req.query
        ),
        Some(payload),
    )))
}
