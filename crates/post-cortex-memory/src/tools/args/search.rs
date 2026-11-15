// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! Arguments of the `semantic_search` tool. The schemars attributes are the tool's
//! published input schema, pinned by the daemon's `tools_list.json`.

use schemars::JsonSchema;
use serde::Deserialize;

/// Request payload for the `semantic_search` tool.
#[derive(Deserialize, JsonSchema, Debug)]
pub struct SemanticSearchRequest {
    /// Natural-language search query.
    #[schemars(description = r#"What to find, in natural language."#)]
    pub query: String,
    /// Search scope (`session`, `workspace`, or `global`).
    #[schemars(
        description = r#"session, workspace or global (default). session and workspace need scope_id."#
    )]
    pub scope: Option<String>,
    /// UUID identifying the session or workspace to search.
    #[schemars(description = r#"The session or workspace UUID, for scope session or workspace."#)]
    pub scope_id: Option<String>,
    /// Maximum number of results to return.
    #[schemars(description = r#"At most this many results (default 10, max 100)."#)]
    pub limit: Option<usize>,
    /// ISO 8601 lower bound for date filtering.
    #[schemars(description = r#"Only results from this date on (YYYY-MM-DD or RFC 3339)."#)]
    pub date_from: Option<String>,
    /// ISO 8601 upper bound for date filtering.
    #[schemars(description = r#"Only results up to this date (YYYY-MM-DD or RFC 3339)."#)]
    pub date_to: Option<String>,
    /// Filter results to these interaction types.
    #[schemars(
        description = r#"Only these types: qa, decision_made, problem_solved, code_change, requirement_added, concept_defined."#
    )]
    pub interaction_type: Option<Vec<String>>,
    /// Temporal decay factor favouring recent content.
    #[schemars(
        description = r#"Favour recent results (default 0: off). Scores decay as e^(-λ·days/365): at 0.5 a year-old result keeps ~61%, at 1.0 ~37%. Reorders, filters nothing; 0.5-1.0 suits recent bugs, 0 timeless docs."#
    )]
    pub recency_bias: Option<f32>,
    /// Ranking effort: `fast`, `local` or `accurate`.
    #[schemars(
        description = r#"Ranking effort (default set by the server): fast (vector + keyword fused, ~40 ms), local (plus a local cross-encoder, ~1 s on CPU), accurate (plus the Voyage reranker when the server has a key, ~0.5 s; else as local)."#
    )]
    pub mode: Option<String>,
    /// What to search: `records`, `messages` or `all`.
    #[schemars(
        description = r#"What to search (default records): records (the structured context), messages (captured conversation turns, each with the turns around it) or all. Use messages for what was said but never logged as a record."#
    )]
    pub sources: Option<String>,
    /// Only results by this author.
    #[schemars(
        description = r#"Only records and messages by this author (optional), e.g. "claude-code", "hermes:eq-lead" or a person's name."#
    )]
    pub author: Option<String>,
    /// Rank this author's results a little higher.
    #[schemars(
        description = r#"Rank this author's records and messages a little higher (optional), e.g. your own agent name; others still appear."#
    )]
    pub prefer_author: Option<String>,
    /// Report how each result was found.
    #[schemars(
        description = r#"Add `signals` to each result: its ranks in the vector, BM25 and graph rankings, the fused and the reranker score (optional, default false)."#
    )]
    #[serde(default)]
    pub trace: Option<bool>,
    /// How much of each result's text.
    #[schemars(description = r#"How much of each result's text (default snippet).

- snippet: a long text as its passage around the query's words (about 400 characters, with full_chars, its whole length), a short one whole
- full: every text whole, and the records and turns around each hit (neighbour_of)

Read one result whole: a record (UUID content_id) with query_conversation_context query_type record, a message (message:<seq>) with manage_messages page, an entity (entity:<name>) with get_entity_context, a document with manage_document get."#)]
    pub detail: Option<String>,
    /// Form A: single JSONPath expression. The matched value is returned
    /// directly instead of the full response payload.
    #[schemars(
        description = r#"JSONPath picking part of the response (e.g. "$.results[*].content_id"). Allowed: $, .field, [n], [-n], [a:b], [*], .*, [0,3,7]. Not with select_map."#
    )]
    pub select: Option<String>,
    /// Form B: map of output-key → JSONPath expression. Returns an object
    /// with the same keys, each containing the match result.
    #[schemars(
        description = r#"Output keys to JSONPaths, as in select (e.g. {"ids": "$.results[*].content_id"}); a key with no match is null. Result fields: content_id, session_id, content_type, text_content, similarity_score, importance_score, combined_score, timestamp. Not with select."#
    )]
    pub select_map: Option<std::collections::HashMap<String, String>>,
}
