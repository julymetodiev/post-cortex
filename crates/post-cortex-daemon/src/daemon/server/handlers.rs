// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! MCP tool-call handlers used by the JSON-RPC `tools/call` route.
//!
//! Each `handle_*` here delegates to `post_cortex_mcp::*` for the real work,
//! mapping the loose JSON `arguments` object into typed parameters and
//! wrapping the result in MCP `content[].type=text` shape. The dispatcher in
//! `super::mcp::handle_tool_call` matches `tool_name` to one of these.

use crate::daemon::server::DaemonServer;
use std::collections::HashMap;
use std::sync::Arc;

// =============================================================================
// Session Management
// =============================================================================

pub(super) async fn handle_create_session(
    server: &Arc<DaemonServer>,
    arguments: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let name = arguments["name"].as_str().map(|s| s.to_string());
    let description = arguments["description"].as_str().map(|s| s.to_string());

    let session_id = server
        .memory_system
        .create_session(name, description)
        .await
        .map_err(|e| format!("Failed to create session: {}", e))?;

    Ok(serde_json::json!({
        "content": [{
            "type": "text",
            "text": format!("Created new session: {}", session_id)
        }]
    }))
}

pub(super) async fn handle_load_session(
    _server: &Arc<DaemonServer>,
    arguments: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    use post_cortex_mcp::load_session;

    let session_id = arguments["session_id"]
        .as_str()
        .ok_or_else(|| "Missing session_id".to_string())?;

    let uuid =
        uuid::Uuid::parse_str(session_id).map_err(|e| format!("Invalid session_id: {}", e))?;

    let result = load_session(uuid)
        .await
        .map_err(|e| format!("Failed to load session: {}", e))?;

    Ok(text_response(&result.message))
}

pub(super) async fn handle_list_sessions(
    _server: &Arc<DaemonServer>,
) -> Result<serde_json::Value, String> {
    use post_cortex_mcp::list_sessions;

    let result = list_sessions()
        .await
        .map_err(|e| format!("Failed to list sessions: {}", e))?;

    Ok(text_response(&result.message))
}

pub(super) async fn handle_search_sessions(
    _server: &Arc<DaemonServer>,
    arguments: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    use post_cortex_mcp::search_sessions;

    let query = arguments["query"]
        .as_str()
        .ok_or_else(|| "Missing query".to_string())?
        .to_string();

    let result = search_sessions(query)
        .await
        .map_err(|e| format!("Failed to search sessions: {}", e))?;

    Ok(text_response(&result.message))
}

pub(super) async fn handle_update_session_metadata(
    _server: &Arc<DaemonServer>,
    arguments: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    use post_cortex_mcp::update_session_metadata;

    let session_id = parse_uuid_arg(arguments, "session_id")?;
    let name = arguments
        .get("name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let description = arguments
        .get("description")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let result = update_session_metadata(session_id, name, description)
        .await
        .map_err(|e| format!("Failed to update metadata: {}", e))?;

    Ok(text_response(&result.message))
}

pub(super) async fn handle_create_checkpoint(
    _server: &Arc<DaemonServer>,
    arguments: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    use post_cortex_mcp::create_session_checkpoint;

    let session_id = parse_uuid_arg(arguments, "session_id")?;

    let result = create_session_checkpoint(session_id)
        .await
        .map_err(|e| format!("Failed to create checkpoint: {}", e))?;

    Ok(text_response(&result.message))
}

// =============================================================================
// Context Operations
// =============================================================================

pub(super) async fn handle_update_context(
    _server: &Arc<DaemonServer>,
    arguments: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    use post_cortex_core::core::context_update::CodeReference;
    use post_cortex_mcp::update_conversation_context;

    let interaction_type = arguments["interaction_type"]
        .as_str()
        .ok_or_else(|| "Missing interaction_type".to_string())?
        .to_string();

    let content_obj = arguments["content"]
        .as_object()
        .ok_or_else(|| "Content must be an object".to_string())?;

    let mut content = HashMap::new();
    for (key, value) in content_obj {
        let value_str = value
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| value.to_string());
        content.insert(key.clone(), value_str);
    }

    let code_reference: Option<CodeReference> = arguments
        .get("code_reference")
        .and_then(|v| serde_json::from_value(v.clone()).ok());

    let session_id = parse_uuid_arg(arguments, "session_id")?;

    let result =
        update_conversation_context(interaction_type, content, code_reference, session_id)
            .await
            .map_err(|e| format!("Failed to update context: {}", e))?;

    Ok(text_response(&result.message))
}

pub(super) async fn handle_query_context(
    _server: &Arc<DaemonServer>,
    arguments: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    use post_cortex_mcp::query_conversation_context;

    let query_type = arguments["query_type"]
        .as_str()
        .ok_or_else(|| "Missing query_type".to_string())?
        .to_string();

    let params_obj = arguments["parameters"]
        .as_object()
        .ok_or_else(|| "Parameters must be an object".to_string())?;

    let mut parameters = HashMap::new();
    for (key, value) in params_obj {
        let value_str = value
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| value.to_string());
        parameters.insert(key.clone(), value_str);
    }

    let session_id = parse_uuid_arg(arguments, "session_id")?;

    let result = query_conversation_context(query_type, parameters, session_id)
        .await
        .map_err(|e| format!("Failed to query context: {}", e))?;

    Ok(text_response(&result.message))
}

pub(super) async fn handle_bulk_update_context(
    _server: &Arc<DaemonServer>,
    arguments: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    use post_cortex_mcp::{ContextUpdateItem, bulk_update_conversation_context};

    let session_id = parse_uuid_arg(arguments, "session_id")?;

    let updates_json = arguments["updates"]
        .as_array()
        .ok_or_else(|| "Updates must be an array".to_string())?;

    let mut updates: Vec<ContextUpdateItem> = Vec::new();
    for update_val in updates_json {
        let update: ContextUpdateItem = serde_json::from_value(update_val.clone())
            .map_err(|e| format!("Failed to parse update: {}", e))?;
        updates.push(update);
    }

    let result = bulk_update_conversation_context(updates, session_id)
        .await
        .map_err(|e| format!("Failed to bulk update: {}", e))?;

    Ok(text_response(&result.message))
}

// =============================================================================
// Semantic Search
// =============================================================================

pub(super) async fn handle_semantic_search(
    _server: &Arc<DaemonServer>,
    arguments: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    use post_cortex_mcp::semantic_search_session;

    let session_id = parse_uuid_arg(arguments, "session_id")?;

    let query = arguments["query"]
        .as_str()
        .ok_or_else(|| "Missing query".to_string())?
        .to_string();

    let (limit, date_from, date_to, interaction_type, recency_bias) =
        parse_search_filters(arguments);

    let result = semantic_search_session(
        session_id,
        query,
        limit,
        date_from,
        date_to,
        interaction_type,
        recency_bias,
    )
    .await
    .map_err(|e| format!("Failed to search: {}", e))?;

    Ok(text_response(&result.message))
}

pub(super) async fn handle_semantic_search_global(
    _server: &Arc<DaemonServer>,
    arguments: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    use post_cortex_mcp::semantic_search_global;

    let query = arguments["query"]
        .as_str()
        .ok_or_else(|| "Missing query".to_string())?
        .to_string();

    let (limit, date_from, date_to, interaction_type, recency_bias) =
        parse_search_filters(arguments);

    let result = semantic_search_global(
        query,
        limit,
        date_from,
        date_to,
        interaction_type,
        recency_bias,
    )
    .await
    .map_err(|e| format!("Failed to search globally: {}", e))?;

    Ok(text_response(&result.message))
}

pub(super) async fn handle_find_related_content(
    _server: &Arc<DaemonServer>,
    arguments: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    use post_cortex_mcp::find_related_content;

    let session_id = parse_uuid_arg(arguments, "session_id")?;

    let topic = arguments["topic"]
        .as_str()
        .ok_or_else(|| "Missing topic".to_string())?
        .to_string();

    let limit = arguments
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize);

    let result = find_related_content(session_id, topic, limit)
        .await
        .map_err(|e| format!("Failed to find related content: {}", e))?;

    Ok(text_response(&result.message))
}

pub(super) async fn handle_vectorize_session(
    _server: &Arc<DaemonServer>,
    arguments: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    use post_cortex_mcp::vectorize_session;

    let session_id = parse_uuid_arg(arguments, "session_id")?;

    let result = vectorize_session(session_id)
        .await
        .map_err(|e| format!("Failed to vectorize session: {}", e))?;

    Ok(text_response(&result.message))
}

pub(super) async fn handle_get_vectorization_stats(
    _server: &Arc<DaemonServer>,
) -> Result<serde_json::Value, String> {
    use post_cortex_mcp::get_vectorization_stats;

    let result = get_vectorization_stats()
        .await
        .map_err(|e| format!("Failed to get vectorization stats: {}", e))?;

    Ok(text_response(&result.message))
}

// =============================================================================
// Analysis & Insights
// =============================================================================

pub(super) async fn handle_get_summary(
    _server: &Arc<DaemonServer>,
    arguments: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    use post_cortex_mcp::get_structured_summary;

    let session_id = arguments["session_id"]
        .as_str()
        .ok_or_else(|| "Missing session_id".to_string())?
        .to_string();

    let compact = arguments.get("compact").and_then(|v| v.as_bool());
    let decisions_limit = arguments
        .get("decisions_limit")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize);
    let entities_limit = arguments
        .get("entities_limit")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize);
    let questions_limit = arguments
        .get("questions_limit")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize);
    let concepts_limit = arguments
        .get("concepts_limit")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize);
    let min_confidence = arguments
        .get("min_confidence")
        .and_then(|v| v.as_f64())
        .map(|v| v as f32);

    let result = get_structured_summary(
        session_id,
        decisions_limit,
        entities_limit,
        questions_limit,
        concepts_limit,
        min_confidence,
        compact,
    )
    .await
    .map_err(|e| format!("Failed to get summary: {}", e))?;

    Ok(text_response(&result.message))
}

pub(super) async fn handle_get_key_decisions(
    _server: &Arc<DaemonServer>,
    arguments: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    use post_cortex_mcp::get_key_decisions;

    let session_id = arguments["session_id"]
        .as_str()
        .ok_or_else(|| "Missing session_id".to_string())?
        .to_string();

    let result = get_key_decisions(session_id)
        .await
        .map_err(|e| format!("Failed to get key decisions: {}", e))?;

    Ok(text_response(&result.message))
}

pub(super) async fn handle_get_key_insights(
    _server: &Arc<DaemonServer>,
    arguments: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    use post_cortex_mcp::get_key_insights;

    let session_id = arguments["session_id"]
        .as_str()
        .ok_or_else(|| "Missing session_id".to_string())?
        .to_string();

    let limit = arguments
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize);

    let result = get_key_insights(session_id, limit)
        .await
        .map_err(|e| format!("Failed to get key insights: {}", e))?;

    Ok(text_response(&result.message))
}

pub(super) async fn handle_get_entity_importance(
    _server: &Arc<DaemonServer>,
    arguments: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    use post_cortex_mcp::get_entity_importance_analysis;

    let session_id = arguments["session_id"]
        .as_str()
        .ok_or_else(|| "Missing session_id".to_string())?
        .to_string();

    let limit = arguments
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize);
    let min_importance = arguments
        .get("min_importance")
        .and_then(|v| v.as_f64())
        .map(|v| v as f32);

    let result = get_entity_importance_analysis(session_id, limit, min_importance)
        .await
        .map_err(|e| format!("Failed to get entity importance: {}", e))?;

    Ok(text_response(&result.message))
}

pub(super) async fn handle_get_entity_network(
    _server: &Arc<DaemonServer>,
    arguments: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    use post_cortex_mcp::get_entity_network_view;

    let session_id = arguments["session_id"]
        .as_str()
        .ok_or_else(|| "Missing session_id".to_string())?
        .to_string();

    let center_entity = arguments
        .get("center_entity")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let max_entities = arguments
        .get("max_entities")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize);
    let max_relationships = arguments
        .get("max_relationships")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize);

    let result =
        get_entity_network_view(session_id, center_entity, max_entities, max_relationships)
            .await
            .map_err(|e| format!("Failed to get entity network: {}", e))?;

    Ok(text_response(&result.message))
}

pub(super) async fn handle_get_session_statistics(
    _server: &Arc<DaemonServer>,
    arguments: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    use post_cortex_mcp::get_session_statistics;

    let session_id = arguments["session_id"]
        .as_str()
        .ok_or_else(|| "Missing session_id".to_string())?
        .to_string();

    let result = get_session_statistics(session_id)
        .await
        .map_err(|e| format!("Failed to get statistics: {}", e))?;

    Ok(text_response(&result.message))
}

pub(super) async fn handle_get_tool_catalog(
    _server: &Arc<DaemonServer>,
) -> Result<serde_json::Value, String> {
    use post_cortex_mcp::get_tool_catalog;

    let result = get_tool_catalog()
        .await
        .map_err(|e| format!("Failed to get tool catalog: {}", e))?;

    Ok(text_response(&result.message))
}

// =============================================================================
// Workspace Management
// =============================================================================

pub(super) async fn handle_create_workspace(
    _server: &Arc<DaemonServer>,
    arguments: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    use post_cortex_mcp::create_workspace;

    let name = arguments["name"]
        .as_str()
        .ok_or_else(|| "Missing name".to_string())?
        .to_string();

    let description = arguments["description"]
        .as_str()
        .ok_or_else(|| "Missing description".to_string())?
        .to_string();

    let result = create_workspace(name, description)
        .await
        .map_err(|e| format!("Failed to create workspace: {}", e))?;

    Ok(text_response(&result.message))
}

pub(super) async fn handle_get_workspace(
    _server: &Arc<DaemonServer>,
    arguments: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    use post_cortex_mcp::get_workspace;

    let workspace_id = parse_uuid_arg(arguments, "workspace_id")?;

    let result = get_workspace(workspace_id)
        .await
        .map_err(|e| format!("Failed to get workspace: {}", e))?;

    Ok(text_response(&result.message))
}

pub(super) async fn handle_list_workspaces(
    _server: &Arc<DaemonServer>,
) -> Result<serde_json::Value, String> {
    use post_cortex_mcp::list_workspaces;

    let result = list_workspaces()
        .await
        .map_err(|e| format!("Failed to list workspaces: {}", e))?;

    Ok(text_response(&result.message))
}

pub(super) async fn handle_delete_workspace(
    _server: &Arc<DaemonServer>,
    arguments: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    use post_cortex_mcp::delete_workspace;

    let workspace_id = parse_uuid_arg(arguments, "workspace_id")?;

    let result = delete_workspace(workspace_id)
        .await
        .map_err(|e| format!("Failed to delete workspace: {}", e))?;

    Ok(text_response(&result.message))
}

pub(super) async fn handle_add_session_to_workspace(
    _server: &Arc<DaemonServer>,
    arguments: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    use post_cortex_mcp::add_session_to_workspace;

    let workspace_id = parse_uuid_arg(arguments, "workspace_id")?;
    let session_id = parse_uuid_arg(arguments, "session_id")?;

    let role = arguments["role"]
        .as_str()
        .ok_or_else(|| "Missing role".to_string())?
        .to_string();

    let result = add_session_to_workspace(workspace_id, session_id, role)
        .await
        .map_err(|e| format!("Failed to add session to workspace: {}", e))?;

    Ok(text_response(&result.message))
}

pub(super) async fn handle_remove_session_from_workspace(
    _server: &Arc<DaemonServer>,
    arguments: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    use post_cortex_mcp::remove_session_from_workspace;

    let workspace_id = parse_uuid_arg(arguments, "workspace_id")?;
    let session_id = parse_uuid_arg(arguments, "session_id")?;

    let result = remove_session_from_workspace(workspace_id, session_id)
        .await
        .map_err(|e| format!("Failed to remove session from workspace: {}", e))?;

    Ok(text_response(&result.message))
}

// =============================================================================
// Local helpers
// =============================================================================

fn text_response(text: &str) -> serde_json::Value {
    serde_json::json!({
        "content": [{
            "type": "text",
            "text": text,
        }]
    })
}

fn parse_uuid_arg(arguments: &serde_json::Value, key: &str) -> Result<uuid::Uuid, String> {
    let raw = arguments[key]
        .as_str()
        .ok_or_else(|| format!("Missing {}", key))?;
    uuid::Uuid::parse_str(raw).map_err(|e| format!("Invalid {}: {}", key, e))
}

/// Common search-filter extraction shared between session/global semantic search.
fn parse_search_filters(
    arguments: &serde_json::Value,
) -> (
    Option<usize>,
    Option<String>,
    Option<String>,
    Option<Vec<String>>,
    Option<f32>,
) {
    let limit = arguments
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize);
    let date_from = arguments
        .get("date_from")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let date_to = arguments
        .get("date_to")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let interaction_type = arguments
        .get("interaction_type")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect::<Vec<String>>()
        });
    let recency_bias = arguments
        .get("recency_bias")
        .and_then(|v| v.as_f64())
        .map(|v| v as f32);

    (limit, date_from, date_to, interaction_type, recency_bias)
}
