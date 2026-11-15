// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! Arguments of the `query_conversation_context` tool. The schemars attributes are the tool's
//! published input schema, pinned by the daemon's `tools_list.json`.

use schemars::JsonSchema;
use serde::Deserialize;

/// Request payload for the `query_conversation_context` tool.
#[derive(Deserialize, JsonSchema, Debug)]
pub struct QueryConversationContextRequest {
    /// Session UUID to query.
    #[schemars(description = "Session ID")]
    pub session_id: String,
    /// Query type selector.
    #[schemars(
        description = "Query type. Supported: find_related_entities, get_entity_context, search_updates, entity_importance, entity_network, find_related_content (topic, max_results), key_decisions, key_insights (limit), session_statistics, structured_summary, decisions, open_questions, related_entities, all_entities, trace_relationships, get_most_important_entities, get_recently_mentioned_entities, analyze_entity_importance, find_entities_by_type, assemble_context, recent_changes, code_references, record (entry_id: one record whole, e.g. one a semantic_search snippet cut). Graph views (JSON; add workspace_id to view a whole workspace): entity (entity, limit: records, decisions, relations, aliases), impact (entity, depth: who depends on it plus valid decisions/requirements/problems about them, a pre-change risk report), topics (limit, per_topic: entity communities with key decisions), lineage (entity, depth: problem -> decision -> consequences in time order), radar (limit, include_dismissed: unsolved problems, decisions without rationale, open questions, stale entities, possible aliases; items dismissed with manage_workspace dismiss_radar_item are left out and listed under dismissed unless include_dismissed=true), file (path, depth: past problems, decisions and dependents of a source file)."
    )]
    pub query_type: String,
    /// Key-value query parameters.
    #[schemars(description = "Query parameters as key-value pairs")]
    pub parameters: std::collections::HashMap<String, String>,
}
