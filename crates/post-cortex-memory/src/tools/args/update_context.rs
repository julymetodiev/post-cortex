// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! Arguments of the `update_conversation_context` tool. The schemars attributes are the tool's
//! published input schema, pinned by the daemon's `tools_list.json`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// One update of a bulk write.
#[derive(Deserialize, JsonSchema, Debug, Clone)]
pub struct ContextUpdateItem {
    /// Interaction type classifier (e.g., `qa`, `decision_made`).
    #[schemars(
        description = r#"One of qa, decision_made, problem_solved, code_change, requirement_added, concept_defined (exact, lowercase)."#
    )]
    pub interaction_type: String,
    /// Key-value content payload for the update.
    #[schemars(
        description = r#"Key-value pairs; every value is a string (stringify nested JSON). Its title and description come from the type's keys: qa question, answer; decision_made decision, rationale; problem_solved problem, solution; code_change file (or file_path), changes (or diff, change); requirement_added requirement, description; concept_defined concept, definition. title and description stand in for either; at least one of the two must be given. Other keys are kept as details."#
    )]
    pub content: std::collections::HashMap<String, String>,
    /// Named entities mentioned in this update. Required by the canonical
    /// write path so the entity graph is never silently empty.
    #[serde(default)]
    #[schemars(description = r#"The entities it mentions; at least one."#)]
    pub entities: Vec<EntityItem>,
    /// Relations between the entities listed above.
    #[serde(default)]
    #[schemars(description = r#"Relations between those entities; at least one."#)]
    pub relations: Vec<RelationItem>,
    /// Optional code location reference.
    #[schemars(
        description = r#"Optional code location: "src/main.rs:42" or {"file", "line", "function"}."#
    )]
    pub code_reference: Option<serde_json::Value>,
    /// Captured messages the update was drawn from.
    #[schemars(
        description = r#"Optional: content ids of this session's captured messages it was drawn from (e.g. "message:12"); each must exist."#
    )]
    #[serde(default)]
    pub derived_from: Option<Vec<String>>,
    /// When the fact started to hold.
    #[schemars(
        description = r#"When the fact started to hold, if not now (RFC 3339 or YYYY-MM-DD); optional."#
    )]
    #[serde(default)]
    pub valid_from: Option<String>,
    /// Earlier records this one replaces.
    #[schemars(
        description = r#"Optional: entry ids of this session's records it replaces; they stay in the history, marked as ended."#
    )]
    #[serde(default)]
    pub supersedes: Option<Vec<String>>,
    /// How sure the writer is.
    #[schemars(description = r#"How sure you are, 0..1; optional."#)]
    #[serde(default)]
    pub confidence: Option<f32>,
}

/// Request payload for the `update_conversation_context` tool.
#[derive(Deserialize, JsonSchema, Debug)]
pub struct UpdateConversationContextRequest {
    /// Session UUID to add context to.
    #[schemars(description = r#"Session UUID (from the session tool or search results)."#)]
    pub session_id: String,
    /// Interaction type for single-update mode.
    #[schemars(
        description = r#"Single update: one of qa, decision_made, problem_solved, code_change, requirement_added, concept_defined (exact, lowercase). Required unless updates is given."#
    )]
    pub interaction_type: Option<String>,
    /// Key-value content for single-update mode.
    #[schemars(
        description = r#"Single update: key-value pairs; every value is a string (stringify nested JSON). Its title and description come from the type's keys: qa question, answer; decision_made decision, rationale; problem_solved problem, solution; code_change file (or file_path), changes (or diff, change); requirement_added requirement, description; concept_defined concept, definition. title and description stand in for either; at least one of the two must be given. Other keys are kept as details. Required unless updates is given."#
    )]
    pub content: Option<std::collections::HashMap<String, String>>,
    /// Named entities mentioned in this update (single-update mode).
    /// Required by the canonical write path.
    #[serde(default)]
    #[schemars(description = r#"The entities it mentions; at least one."#)]
    pub entities: Vec<EntityItem>,
    /// Relations between the entities listed above (single-update mode).
    #[serde(default)]
    #[schemars(description = r#"Relations between those entities; at least one."#)]
    pub relations: Vec<RelationItem>,
    /// Optional code reference for single-update mode.
    #[schemars(
        description = r#"Optional code location: "src/main.rs:42" or {"file", "line", "function"}."#
    )]
    pub code_reference: Option<serde_json::Value>,
    /// Captured messages the update was drawn from.
    #[schemars(
        description = r#"Optional: content ids of this session's captured messages it was drawn from (e.g. "message:12"); each must exist."#
    )]
    #[serde(default)]
    pub derived_from: Option<Vec<String>>,
    /// When the fact started to hold.
    #[schemars(
        description = r#"When the fact started to hold, if not now (RFC 3339 or YYYY-MM-DD); optional."#
    )]
    #[serde(default)]
    pub valid_from: Option<String>,
    /// Earlier records this one replaces.
    #[schemars(
        description = r#"Optional: entry ids of this session's records it replaces; they stay in the history, marked as ended."#
    )]
    #[serde(default)]
    pub supersedes: Option<Vec<String>>,
    /// How sure the writer is.
    #[schemars(description = r#"How sure you are, 0..1; optional."#)]
    #[serde(default)]
    pub confidence: Option<f32>,
    /// Bulk update array; overrides single-update fields when present.
    #[schemars(
        description = r#"Bulk: several updates, each with its own interaction_type, content, entities and relations. When given, the single-update fields are ignored."#
    )]
    pub updates: Option<Vec<ContextUpdateItem>>,
    /// Validate without persisting when `true`.
    #[schemars(description = r#"Validate and preview without storing anything."#)]
    pub dry_run: Option<bool>,
    /// Who writes the update(s).
    #[schemars(
        description = r#"Who writes it: your agent name, the user, a teammate (optional). Default: the connection's X-PCX-Author header, else the MCP client's name unless that only names an SDK. Shown in results and usable as a search filter."#
    )]
    pub author: Option<String>,
}

/// An entity an update mentions.
#[derive(Serialize, Deserialize, Debug, JsonSchema, Clone)]
pub struct EntityItem {
    /// Its unique name.
    pub name: String,
    /// concept (the default, also for an unknown value), technology,
    /// problem, solution, decision or code_component.
    #[serde(default = "default_entity_type")]
    pub entity_type: String,
}

fn default_entity_type() -> String {
    "concept".to_string()
}

/// A relation between two entities of the same update.
#[derive(Serialize, Deserialize, Debug, JsonSchema, Clone)]
pub struct RelationItem {
    /// An entity of the update, by name.
    pub from_entity: String,
    /// Another entity of the update, by name.
    pub to_entity: String,
    /// depends_on, implements, caused_by, leads_to, related_to, required_by,
    /// conflicts_with or solves; another value is refused.
    pub relation_type: String,
    /// Why it holds.
    #[schemars(
        length(min = 1),
        description = "Why it holds; required: a relation without it is refused, and the whole update with it."
    )]
    pub context: String,
}
