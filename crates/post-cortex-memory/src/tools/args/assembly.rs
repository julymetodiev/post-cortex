// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! Arguments of the `assemble_context` tool. The schemars attributes are the tool's
//! published input schema, pinned by the daemon's `tools_list.json`.

use schemars::JsonSchema;
use serde::Deserialize;

/// Request payload for the `assemble_context` tool.
#[derive(Deserialize, JsonSchema, Debug)]
pub struct AssembleContextRequest {
    /// Optional session UUID; ignored when `workspace_id` is set.
    #[schemars(description = r#"Session UUID; required without workspace_id."#)]
    pub session_id: Option<String>,
    /// Optional workspace UUID; takes precedence over `session_id`.
    #[schemars(
        description = r#"Workspace UUID: merges the context of all its sessions; takes precedence over session_id."#
    )]
    pub workspace_id: Option<String>,
    /// Query topic used to seed entity extraction and graph traversal.
    #[schemars(description = r#"The question or topic; its entities seed the graph traversal."#)]
    pub query: String,
    /// Maximum token budget for assembled context output.
    #[schemars(
        description = r#"Token budget (default 4000, > 0): items are packed highest score first until it is used."#
    )]
    pub token_budget: Option<u32>,
    /// What to assemble from: `records`, `messages` or `all`.
    #[schemars(
        description = r#"What to assemble from (default records): records (the records, entity graph and impact analysis), messages (captured turns matching the query with the turns around them, plus the graph and impact analysis) or all (records get half the budget)."#
    )]
    pub sources: Option<String>,
    /// Form A: single JSONPath expression. The matched value is returned
    /// directly instead of the full response payload.
    #[schemars(
        description = r#"JSONPath picking part of the response (e.g. "$.formatted_text", "$.items[0:3].text"). Allowed: $, .field, [n], [-n], [a:b], [*], .*, [0,3,7]. Not with select_map."#
    )]
    pub select: Option<String>,
    /// Form B: map of output-key → JSONPath expression. Returns an object
    /// with the same keys, each containing the match result.
    #[schemars(
        description = r#"Output keys to JSONPaths, as in select (e.g. {"text": "$.formatted_text", "ids": "$.items[*].entry_id"}); a key with no match is null. Not with select."#
    )]
    pub select_map: Option<std::collections::HashMap<String, String>>,
}
