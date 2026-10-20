// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! Request payload structs for the 9 consolidated MCP tools.
//!
//! All schemars-decorated fields are kept verbatim so the JSON Schema that
//! `#[tool(input_schema = ...)]` emits matches the previous monolith — clients
//! see the same hints.

use schemars::JsonSchema;
use serde::Deserialize;

// =============================================================================
// Tool 7: assemble_context
// =============================================================================

/// Request payload for the `assemble_context` tool.
#[derive(Deserialize, JsonSchema, Debug)]
pub struct AssembleContextRequest {
    /// Optional session UUID; ignored when `workspace_id` is set.
    #[schemars(
        description = r#"Session UUID. Required when workspace_id is not provided.

Format: xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx

Note: If both session_id and workspace_id are provided, workspace_id takes precedence
(context is merged across all sessions in the workspace)."#
    )]
    pub session_id: Option<String>,
    /// Optional workspace UUID; takes precedence over `session_id`.
    #[schemars(
        description = r#"Workspace UUID. When provided, context is merged across all sessions in the workspace.

Format: xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx

Note: Takes precedence over session_id."#
    )]
    pub workspace_id: Option<String>,
    /// Query topic used to seed entity extraction and graph traversal.
    #[schemars(description = r#"The query/topic to assemble context for.

Example: "How does authentication interact with the user profile service?"

Note: Entity extraction runs on this query to seed graph traversal."#)]
    pub query: String,
    /// Maximum token budget for assembled context output.
    #[schemars(
        description = r#"Token budget for the assembled context (default: 4000).

Examples:
- 2000: tight context window
- 4000: default (matches Axon's per-call budget)
- 8000: extended chains / orchestration

Note: Must be > 0. Items are packed highest-score-first until budget is consumed."#
    )]
    pub token_budget: Option<u32>,
    /// What to assemble from: `records`, `messages` or `all`.
    #[schemars(description = r#"What to assemble from (optional; default: records).

- records: the structured records, entity graph and impact analysis
- messages: conversation excerpts (captured turns matching the query, with the turns around them), plus the entity graph and impact analysis
- all: both; records get half of the token budget"#)]
    pub sources: Option<String>,
    /// Form A: single JSONPath expression. The matched value is returned
    /// directly instead of the full response payload.
    #[schemars(
        description = r#"Optional JSONPath expression to filter the response (Form A).

When provided, only the matched portion of the response is returned,
dramatically reducing payload size.

Allowed syntax: $, .field, [n], [-n], [a:b], [*], .*, [0,3,7]
Forbidden: .., [?(...)], functions, [a:b:c]

Examples:
- "$.formatted_text" → returns the pre-formatted text string
- "$.items[0].text" → returns the first item's text
- "$.items[0:3].text" → returns the first 3 item texts as array
- "$.entity_context[*].name" → returns all entity names as array

Note: Mutually exclusive with select_map."#
    )]
    pub select: Option<String>,
    /// Form B: map of output-key → JSONPath expression. Returns an object
    /// with the same keys, each containing the match result.
    #[schemars(
        description = r#"Optional map of output keys to JSONPath expressions (Form B).

When provided, returns an object with the same keys, each containing
the JSONPath match result. Empty matches produce null.

Allowed syntax: same as select (Form A).

Examples:
- {"text": "$.formatted_text", "top3": "$.items[0:3].text", "ids": "$.items[*].entry_id"}
- {"neighbours": "$.entity_context[0:5].name", "tokens": "$.total_tokens"}

Note: Mutually exclusive with select (Form A)."#
    )]
    pub select_map: Option<std::collections::HashMap<String, String>>,
}

// =============================================================================
// Tool 8: manage_entity
// =============================================================================

/// Request payload for the `manage_entity` tool.
#[derive(Deserialize, JsonSchema, Debug)]
pub struct ManageEntityRequest {
    /// Entity management action (`delete` or `delete_update`).
    #[schemars(description = r#"Action to perform on session content.

Valid values:
- delete: Remove an entity from the session (cascades typed edges). Requires entity_name.
- delete_update: Remove a single context update (ghost / bad row) by entry_id. Requires entry_id.

Note: Must be lowercase. Additional actions may be added later."#)]
    pub action: String,
    /// Session UUID that contains the target entity or update.
    #[schemars(description = r#"Session UUID containing the entity/update.

Format: xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx"#)]
    pub session_id: String,
    /// Entity name for the `delete` action.
    #[schemars(
        description = r#"Entity name to operate on (required for action=delete).

Example: "RocksDB"

Note: Names are matched case-insensitively in the entity graph."#
    )]
    pub entity_name: Option<String>,
    /// Context-update ID for the `delete_update` action.
    #[schemars(
        description = r#"Context-update id (required for action=delete_update).

Format: xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx

Note: This is the `entry_id` returned by `assemble_context` items, or the id of a
ContextUpdate stored in the session. Use it to remove ghost / mis-shaped writes."#
    )]
    pub entry_id: Option<String>,
}

// =============================================================================
// Tool 9: admin
// =============================================================================

/// Request payload for the `admin` tool.
#[derive(Deserialize, JsonSchema, Debug)]
pub struct AdminRequest {
    /// Administrative action (`health`, `vectorize_session`, `vectorize_stats`, `create_checkpoint`).
    #[schemars(description = r#"Administrative action on the Post-Cortex daemon.

Valid values:
- health: Report daemon health (active sessions, embeddings status, version)
- vectorize_session: Backfill embeddings for an existing session (requires session_id)
- vectorize_stats: Return embedding-pipeline statistics as JSON
- create_checkpoint: Persist a snapshot of a session for later recall (requires session_id)

Examples:
✅ {"action": "health"}
✅ {"action": "vectorize_session", "session_id": "60c598e2-..."}
✅ {"action": "vectorize_stats"}
✅ {"action": "create_checkpoint", "session_id": "60c598e2-..."}

Note: Must be lowercase."#)]
    pub action: String,
    /// Session UUID for actions that target a specific session.
    #[schemars(
        description = r#"Session UUID for vectorize_session and create_checkpoint.

Format: xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx"#
    )]
    pub session_id: Option<String>,
}

// =============================================================================
// Tool 1: session
// =============================================================================

/// Request payload for the `session` tool.
#[derive(Deserialize, JsonSchema, Debug)]
pub struct SessionRequest {
    /// Session management action (`create`, `list`, `load`, `search`, `update_metadata`, `delete`).
    #[schemars(description = r#"Action to perform on sessions.

Valid values:
- create: Create a new session and return a UUID (optional: name, description)
- list: List all existing sessions
- load: Load metadata for a single session (requires session_id)
- search: Find sessions by name or description substring (requires query)
- update_metadata: Update session name and/or description (requires session_id; provide name and/or description)
- delete: Permanently delete a session and its updates (requires session_id)

Examples:
✅ {"action": "create", "name": "Auth", "description": "OAuth2 work"}
✅ {"action": "load", "session_id": "60c598e2-..."}
✅ {"action": "search", "query": "auth"}
✅ {"action": "update_metadata", "session_id": "60c598e2-...", "name": "New name"}
✅ {"action": "delete", "session_id": "60c598e2-..."}

Note: Must be lowercase."#)]
    pub action: String,
    /// Session name (used by `create` and `update_metadata`).
    #[schemars(description = r#"Session name (used by create and update_metadata).

Examples:
- "Feature: Authentication"
- "Bug Fix: Memory Leak"

Note: For update_metadata, only provided fields are changed."#)]
    pub name: Option<String>,
    /// Session description (used by `create` and `update_metadata`).
    #[schemars(
        description = r#"Session description (used by create and update_metadata).

Example: "Working on implementing OAuth2 login flow"

Note: For update_metadata, only provided fields are changed."#
    )]
    pub description: Option<String>,
    /// Session UUID (required for `load`, `update_metadata`, `delete`).
    #[schemars(
        description = r#"Session UUID (required for load, update_metadata, delete).

Format: xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx

Example: "60c598e2-d602-4e07-a328-c458006d48c7""#
    )]
    pub session_id: Option<String>,
    /// Search query for the `search` action.
    #[schemars(description = r#"Search query for the search action.

Examples:
- "authentication"
- "image pipeline"

Note: Matches session name or description (case-insensitive substring)."#)]
    pub query: Option<String>,
}

// =============================================================================
// Tool 2: update_conversation_context (single + bulk)
// =============================================================================

/// Single context-update item used in bulk mode.
#[derive(Deserialize, JsonSchema, Debug, Clone)]
pub struct ContextUpdateItem {
    /// Interaction type classifier (e.g., `qa`, `decision_made`).
    #[schemars(description = r#"Type of interaction (must be exact lowercase value).

Valid values:
- qa: Questions and answers about the codebase
- decision_made: Architectural decisions and trade-offs
- problem_solved: Bug fixes and solutions to technical problems
- code_change: Code modifications and refactoring
- requirement_added: New requirements or constraints
- concept_defined: Technical concepts and patterns explained

Examples:
✅ "decision_made" (correct)
❌ "DecisionMade" (wrong - must be lowercase)
❌ "made_decision" (wrong - use exact term)"#)]
    pub interaction_type: String,
    /// Key-value content payload for the update.
    #[schemars(
        description = r#"Content as key-value pairs (all values must be strings).

Format: HashMap<String, String> where both keys and values are strings.

Examples:
Simple: {"decision": "Use Rust", "rationale": "Performance"}
Complex: {"criteria": "{\"performance\": 9, \"safety\": 10}", "date": "2025-01-12"}

Note: For complex nested data, stringify as JSON first. Do not pass nested objects directly."#
    )]
    pub content: std::collections::HashMap<String, String>,
    /// Named entities mentioned in this update. Required by the canonical
    /// write path so the entity graph is never silently empty.
    #[serde(default)]
    #[schemars(description = r#"Named entities mentioned in this update.

Each entry: {"name": "<unique name>", "entity_type": "concept|technology|problem|solution|decision|code_component"}.
Required (must contain at least one entity) so the entity graph captures every write."#)]
    pub entities: Vec<post_cortex_mcp::EntityItem>,
    /// Relations between the entities listed above.
    #[serde(default)]
    #[schemars(description = r#"Relations between the entities listed above.

Each entry: {"from_entity": "<name>", "to_entity": "<name>", "relation_type": "depends_on|implements|caused_by|leads_to|related_to|required_by|conflicts_with|solves", "context": "<short why>"}.
Required (must contain at least one relation). Both endpoints must appear in the `entities` array; self-relations and dangling references are rejected with InvalidArgument."#)]
    pub relations: Vec<post_cortex_mcp::RelationItem>,
    /// Optional code location reference.
    #[schemars(description = r#"Optional code reference for context.

Can be a simple string or complex object:

Examples:
- Simple: "src/main.rs:42"
- Complex: {"file": "src/main.rs", "line": 42, "function": "process_data"}

Note: Helps link context to specific code locations."#)]
    pub code_reference: Option<serde_json::Value>,
    /// Captured messages the update was drawn from.
    #[schemars(
        description = r#"Captured messages of this session the update was drawn from, as their content ids
from search results (e.g. ["message:12", "message:13"]); optional. Each must exist."#
    )]
    #[serde(default)]
    pub derived_from: Option<Vec<String>>,
}

/// Request payload for the `update_conversation_context` tool.
#[derive(Deserialize, JsonSchema, Debug)]
pub struct UpdateConversationContextRequest {
    /// Session UUID to add context to.
    #[schemars(description = r#"Session ID (36-char UUID format with hyphens).

Format: xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx

How to get:
1. Create session: Use session tool with action='create'
2. Find session: Use semantic_search to search for previous sessions

Example: "60c598e2-d602-4e07-a328-c458006d48c7"

Note: Must be a string, not a number. UUIDs are always strings."#)]
    pub session_id: String,
    /// Interaction type for single-update mode.
    #[schemars(
        description = r#"Type of interaction for single update (must be exact lowercase value).

Valid values:
- qa: Questions and answers about the codebase
- decision_made: Architectural decisions and trade-offs
- problem_solved: Bug fixes and solutions to technical problems
- code_change: Code modifications and refactoring
- requirement_added: New requirements or constraints
- concept_defined: Technical concepts and patterns explained

Note: Required for single update mode (when 'updates' array is not provided)."#
    )]
    pub interaction_type: Option<String>,
    /// Key-value content for single-update mode.
    #[schemars(
        description = r#"Content as key-value pairs for single update (all values must be strings).

Format: HashMap<String, String> where both keys and values are strings.

Examples:
Simple: {"decision": "Use Rust", "rationale": "Performance"}
Complex: {"criteria": "{\"performance\": 9}", "date": "2025-01-12"}

Note: For complex nested data, stringify as JSON first. Required for single update mode."#
    )]
    pub content: Option<std::collections::HashMap<String, String>>,
    /// Named entities mentioned in this update (single-update mode).
    /// Required by the canonical write path.
    #[serde(default)]
    #[schemars(description = r#"Named entities mentioned in this update.

Each entry: {"name": "<unique name>", "entity_type": "concept|technology|problem|solution|decision|code_component"}.
Required (must contain at least one entity) so the entity graph captures every write."#)]
    pub entities: Vec<post_cortex_mcp::EntityItem>,
    /// Relations between the entities listed above (single-update mode).
    #[serde(default)]
    #[schemars(description = r#"Relations between the entities listed above.

Each entry: {"from_entity": "<name>", "to_entity": "<name>", "relation_type": "depends_on|implements|caused_by|leads_to|related_to|required_by|conflicts_with|solves", "context": "<short why>"}.
Required (must contain at least one relation). Both endpoints must appear in the `entities` array; self-relations and dangling references are rejected with InvalidArgument."#)]
    pub relations: Vec<post_cortex_mcp::RelationItem>,
    /// Optional code reference for single-update mode.
    #[schemars(description = r#"Optional code reference for single update.

Can be a simple string or complex object:

Examples:
- Simple: "src/main.rs:42"
- Complex: {"file": "src/main.rs", "line": 42, "function": "process_data"}"#)]
    pub code_reference: Option<serde_json::Value>,
    /// Captured messages the update was drawn from.
    #[schemars(
        description = r#"Captured messages of this session the update was drawn from, as their content ids
from search results (e.g. ["message:12", "message:13"]); optional. Each must exist."#
    )]
    #[serde(default)]
    pub derived_from: Option<Vec<String>>,
    /// Bulk update array; overrides single-update fields when present.
    #[schemars(
        description = r#"Array of updates for bulk operation (overrides single fields if provided).

Use this mode to add multiple updates at once.

Format: Array of objects, each with interaction_type and content.

Example:
{
  "session_id": "60c598e2-d602-4e07-a328-c458006d48c7",
  "updates": [
    {
      "interaction_type": "decision_made",
      "content": {"decision": "Use Rust", "rationale": "Performance"}
    },
    {
      "interaction_type": "code_change",
      "content": {"file": "main.rs", "change": "Add error handling"}
    }
  ]
}

Note: When provided, 'interaction_type' and 'content' fields are ignored."#
    )]
    pub updates: Option<Vec<ContextUpdateItem>>,
    /// Validate without persisting when `true`.
    #[schemars(
        description = r#"If true, validate the request without making any changes.

When dry_run is true, the request will be validated and a preview of what would happen will be returned, but no data will be stored.

Use cases:
- Test if your request format is correct
- Preview what would be stored
- Validate interaction_type and content structure
- Check if session exists

Example: {"dry_run": true, "session_id": "...", "interaction_type": "decision_made", "content": {...}}

Note: No changes are made to the session when dry_run is true."#
    )]
    pub dry_run: Option<bool>,
    /// Who writes the update(s).
    #[schemars(
        description = r#"Who writes the update(s): your agent name, the user, a teammate (optional).
Defaults to the MCP client's name (e.g. "claude-code"). Shown with search results and usable as a search filter."#
    )]
    pub author: Option<String>,
}

// =============================================================================
// Tool 3: semantic_search (unified)
// =============================================================================

/// Request payload for the `semantic_search` tool.
#[derive(Deserialize, JsonSchema, Debug)]
pub struct SemanticSearchRequest {
    /// Natural-language search query.
    #[schemars(description = r#"Search query for semantic search.

Examples:
- "How did we handle authentication?"
- "What were the performance issues?"
- "decision_made API design"

Note: Natural language queries work best. The search understands context."#)]
    pub query: String,
    /// Search scope (`session`, `workspace`, or `global`).
    #[schemars(
        description = r#"Search scope: 'session', 'workspace', or 'global' (default: 'global').

Valid values:
- session: Search within a specific session (requires scope_id)
- workspace: Search within a workspace (requires scope_id)
- global: Search across all data (default, no scope_id needed)

Examples:
✅ {"query": "performance", "scope": "global"} (default, searches everywhere)
✅ {"query": "auth", "scope": "session", "scope_id": "60c598e2-d602-4e07-a328-c458006d48c7"} (search specific session)
✅ {"query": "API", "scope": "workspace", "scope_id": "f1d2e3a4-b5c6-7d8e-9f0a-1b2c3d4e5f6f"} (search specific workspace)

Note: scope must be lowercase. When using 'session' or 'workspace', you must provide scope_id."#
    )]
    pub scope: Option<String>,
    /// UUID identifying the session or workspace to search.
    #[schemars(
        description = r#"Session ID or Workspace ID (required when scope is 'session' or 'workspace').

Format: 36-char UUID string (xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx)

When to use:
- Required when scope='session' (provide session UUID)
- Required when scope='workspace' (provide workspace UUID)
- Ignored when scope='global' or not specified

Examples:
✅ {"scope": "session", "scope_id": "60c598e2-d602-4e07-a328-c458006d48c7"} (correct)
❌ {"scope": "session"} (missing scope_id - will error)
❌ {"scope": "global", "scope_id": "..."} (scope_id ignored for global)

Note: Must be a valid UUID string. Use 'session' or 'semantic_search' tools to find UUIDs."#
    )]
    pub scope_id: Option<String>,
    /// Maximum number of results to return.
    #[schemars(
        description = r#"Maximum number of results to return (default: 10, max: 100).

Examples:
- 5: Return top 5 most relevant results
- 20: Return top 20 results
- null or omit: Use default of 10

Note: Higher values may slow down search. Maximum allowed is 100."#
    )]
    pub limit: Option<usize>,
    /// ISO 8601 lower bound for date filtering.
    #[schemars(
        description = r#"Filter results from this date onwards (ISO 8601 format).

Format: YYYY-MM-DD or YYYY-MM-DDTHH:MM:SSZ

Examples:
- "2025-01-01": Results from January 1st, 2025 onwards
- "2025-01-15T10:00:00Z": Results from January 15th, 2025 at 10am UTC onwards

Note: Use with date_to to specify a date range."#
    )]
    pub date_from: Option<String>,
    /// ISO 8601 upper bound for date filtering.
    #[schemars(description = r#"Filter results up to this date (ISO 8601 format).

Format: YYYY-MM-DD or YYYY-MM-DDTHH:MM:SSZ

Examples:
- "2025-01-31": Results up to January 31st, 2025
- "2025-01-15T10:00:00Z": Results up to January 15th, 2025 at 10am UTC

Note: Use with date_from to specify a date range."#)]
    pub date_to: Option<String>,
    /// Filter results to these interaction types.
    #[schemars(description = r#"Filter by interaction types (array of strings).

Valid types:
- qa: Questions and answers
- decision_made: Architectural decisions
- problem_solved: Bug fixes and solutions
- code_change: Code modifications
- requirement_added: New requirements
- concept_defined: Technical concepts

Examples:
- ["decision_made"]: Only show decisions
- ["decision_made", "problem_solved"]: Show decisions and solutions
- null or omit: Show all interaction types

Note: Must be exact lowercase values. Filters reduce results to only these types."#)]
    pub interaction_type: Option<Vec<String>>,
    /// Temporal decay factor favouring recent content.
    #[schemars(
        description = r#"Temporal decay factor to prioritize recent content (default: 0.0 = disabled).

Valid values:
- 0.0: Disabled - pure relevance ranking (default, backward compatible)
- 0.1 - 0.5: Soft bias toward recent content
- 0.5 - 1.0: Moderate bias toward recent content
- 1.0+: Aggressive bias toward recent content

How it works:
- Uses exponential decay: score × e^(-λ × days/365)
- Older content gets progressively lower scores
- Fresh content (1 day) retains ~99.9% score at λ=0.5
- Year-old content retains ~60.6% score at λ=0.5, ~36.8% at λ=1.0

When to use:
- Debugging recent issues: 0.5 - 1.0
- Finding latest solutions: 0.3 - 0.7
- Architecture docs (timeless): 0.0 or omit
- Current context: 1.0+

Examples:
✅ {"query": "timeout error", "recency_bias": 0.5} (recent bugs prioritized)
✅ {"query": "authentication", "recency_bias": 0.0} (all docs equal)
✅ {"query": "performance", "recency_bias": 1.0} (very recent)

Note: Only affects ranking order, doesn't filter out old results."#
    )]
    pub recency_bias: Option<f32>,
    /// Ranking effort: `fast`, `local` or `accurate`.
    #[schemars(description = r#"Ranking effort (optional; default set by the server).

- fast: vector + keyword search fused, no reranking (~40 ms)
- local: plus a local cross-encoder reranker (~1 s on CPU)
- accurate: plus the Voyage reranker if the server has a key (~0.5 s), else as local

Use fast for quick lookups; accurate when the answer must not be missed."#)]
    pub mode: Option<String>,
    /// What to search: `records`, `messages` or `all`.
    #[schemars(description = r#"What to search (optional; default: records).

- records: the structured context (decisions, problems, code changes, ...)
- messages: raw conversation turns captured by client hooks, each returned with the turns around it
- all: both

Use messages to find what was said in a conversation that was never logged as a record."#)]
    pub sources: Option<String>,
    /// Only results by this author.
    #[schemars(
        description = r#"Only records and messages by this author (optional), e.g. "claude-code",
"hermes:eq-lead" or a person's name. Results show their author when known."#
    )]
    pub author: Option<String>,
    /// Rank this author's results a little higher.
    #[schemars(
        description = r#"Rank records and messages by this author a little higher (optional), e.g. your
own agent name to favour your own past work. Unlike author, others' results still appear."#
    )]
    pub prefer_author: Option<String>,
    /// Form A: single JSONPath expression. The matched value is returned
    /// directly instead of the full response payload.
    #[schemars(
        description = r#"Optional JSONPath expression to filter the response (Form A).

When provided, only the matched portion of the response is returned,
dramatically reducing payload size — perfect for trimming what the LLM
sees in tool_result.

Allowed syntax: $, .field, [n], [-n], [a:b], [*], .*, [0,3,7]
Forbidden: .., [?(...)], functions, [a:b:c]

Examples:
- "$.results[0:3].text_content" → returns the first 3 result texts as array
- "$.results[*].content_id" → returns all content IDs as array
- "$.results[*].similarity_score" → returns the scores

Note: Mutually exclusive with select_map."#
    )]
    pub select: Option<String>,
    /// Form B: map of output-key → JSONPath expression. Returns an object
    /// with the same keys, each containing the match result.
    #[schemars(
        description = r#"Optional map of output keys to JSONPath expressions (Form B).

When provided, returns an object with the same keys, each containing
the JSONPath match result. Empty matches produce null.

Allowed syntax: same as select (Form A).

Examples:
- {"top3": "$.results[0:3].text_content", "ids": "$.results[*].content_id"}
- {"scores": "$.results[*].similarity_score", "first": "$.results[0].text_content"}

Every scope (session, workspace, global) returns the same result fields: content_id,
session_id, content_type, text_content, similarity_score, importance_score,
combined_score, timestamp.

Note: Mutually exclusive with select (Form A)."#
    )]
    pub select_map: Option<std::collections::HashMap<String, String>>,
}

// =============================================================================
// Tool 4: get_structured_summary (extended)
// =============================================================================

/// Request payload for the `get_structured_summary` tool.
#[derive(Deserialize, JsonSchema, Debug)]
pub struct GetStructuredSummaryRequest {
    /// Session UUID to summarize.
    #[schemars(description = "Session ID")]
    pub session_id: String,
    /// Sections to include (`decisions`, `insights`, `entities`, `all`).
    #[schemars(
        description = "Sections to include: decisions, insights, entities, all (default: all)"
    )]
    pub include: Option<Vec<String>>,
    /// Maximum number of decisions to include.
    #[schemars(description = "Maximum decisions to include")]
    pub decisions_limit: Option<usize>,
    /// Maximum number of entities to include.
    #[schemars(description = "Maximum entities to include")]
    pub entities_limit: Option<usize>,
    /// Maximum number of questions to include.
    #[schemars(description = "Maximum questions to include")]
    pub questions_limit: Option<usize>,
    /// Maximum number of concepts to include.
    #[schemars(description = "Maximum concepts to include")]
    pub concepts_limit: Option<usize>,
    /// Minimum confidence level for included items.
    #[schemars(description = "Minimum confidence level")]
    pub min_confidence: Option<f32>,
    /// Whether to use compact output format.
    #[schemars(description = "Use compact format for large sessions")]
    pub compact: Option<bool>,
}

// =============================================================================
// Tool 5: query_conversation_context
// =============================================================================

/// Request payload for the `query_conversation_context` tool.
#[derive(Deserialize, JsonSchema, Debug)]
pub struct QueryConversationContextRequest {
    /// Session UUID to query.
    #[schemars(description = "Session ID")]
    pub session_id: String,
    /// Query type selector.
    #[schemars(
        description = "Query type. Supported: find_related_entities, get_entity_context, search_updates, entity_importance, entity_network, find_related_content (topic, max_results), key_decisions, key_insights (limit), session_statistics, structured_summary, decisions, open_questions, related_entities, all_entities, trace_relationships, get_most_important_entities, get_recently_mentioned_entities, analyze_entity_importance, find_entities_by_type, assemble_context, recent_changes, code_references."
    )]
    pub query_type: String,
    /// Key-value query parameters.
    #[schemars(description = "Query parameters as key-value pairs")]
    pub parameters: std::collections::HashMap<String, String>,
}

// =============================================================================
// Tool 6: manage_workspace
// =============================================================================

/// Request payload for the `manage_workspace` tool.
#[derive(Deserialize, JsonSchema, Debug)]
pub struct ManageWorkspaceRequest {
    /// Workspace action (`create`, `list`, `get`, `delete`, `add_session`, `remove_session`).
    #[schemars(description = r#"Action to perform on workspaces.

Valid values:
- create: Create a new workspace (requires name and description)
- list: List all workspaces
- get: Get workspace details (requires workspace_id)
- delete: Delete a workspace (requires workspace_id)
- add_session: Add a session to workspace (requires workspace_id, session_id, optional role)
- remove_session: Remove a session from workspace (requires workspace_id, session_id)
- get_card: Read the project card (workspace_id) or the user card (card: "user"); optional history
- update_card: Write the next version of a card (content, expected_version; workspace_id or card: "user")

Cards are short markdown notes (max 2 KB) put in front of every session: the project card holds the
stack, conventions and current goals; the user card holds preferences. update_card needs the version
you read (0 for a new card); if someone changed it since, nothing is written and the latest version
comes back so you can merge and retry.

Examples:
✅ {"action": "create", "name": "Auth Feature", "description": "OAuth2 work"}
✅ {"action": "get", "workspace_id": "f1d2e3a4-..."}
✅ {"action": "add_session", "workspace_id": "...", "session_id": "...", "role": "primary"}

Note: Must be lowercase. Different actions require different parameters."#)]
    pub action: String,
    /// Workspace UUID for actions that target a specific workspace.
    #[schemars(
        description = r#"Workspace ID (36-char UUID) for get/delete/add_session/remove_session actions.

Format: xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx

When to use:
- Required for: get, delete, add_session, remove_session
- Not used for: create, list

How to get:
- Use manage_workspace with action='list' to see all workspaces
- Use semantic_search to find workspaces by name

Example: "f1d2e3a4-b5c6-7d8e-9f0a-1b2c3d4e5f6f"

Note: Must be a valid UUID string. Not a number."#
    )]
    pub workspace_id: Option<String>,
    /// Session UUID for `add_session`/`remove_session`.
    #[schemars(
        description = r#"Session ID (36-char UUID) for add_session/remove_session actions.

Format: xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx

When to use:
- Required for: add_session, remove_session
- Not used for: create, list, get, delete

How to get:
- Use session tool with action='list' to see all sessions
- Use semantic_search to find sessions

Example: "60c598e2-d602-4e07-a328-c458006d48c7"

Note: Must be a valid UUID string. Not a number."#
    )]
    pub session_id: Option<String>,
    /// Workspace name (used by `create`).
    #[schemars(description = r#"Workspace name (for create action).

Example: "Authentication Feature" or "API Design"

Note: Only used when action='create'. Helps identify the workspace."#)]
    pub name: Option<String>,
    /// Workspace description (used by `create`).
    #[schemars(description = r#"Workspace description (for create action).

Example: "Working on OAuth2 authentication and user management"

Note: Only used when action='create'. Provides context for the workspace."#)]
    pub description: Option<String>,
    /// Session role within the workspace (used by `add_session`).
    #[schemars(
        description = r#"Session role in workspace (for add_session action, default: 'related').

Valid values:
- primary: Main session for this workspace
- related: Related context session
- dependency: Required dependency session
- shared: Shared reference session

Examples:
✅ {"action": "add_session", "workspace_id": "...", "session_id": "...", "role": "primary"}
✅ {"action": "add_session", "workspace_id": "...", "session_id": "...", "role": "related"}
✅ {"action": "add_session", "workspace_id": "...", "session_id": "..."} (defaults to "related")

Note: Only used when action='add_session'. Defaults to 'related' if omitted."#
    )]
    pub role: Option<String>,
    /// Which card `get_card`/`update_card` act on: `workspace` or `user`.
    #[schemars(
        description = r#"Which card get_card/update_card act on (default: workspace).

- workspace: the project card of workspace_id
- user: the user's global card (no workspace_id needed)"#
    )]
    pub card: Option<String>,
    /// New card text (used by `update_card`).
    #[schemars(
        description = r#"New card text, markdown, at most 2 KB (for update_card). Replaces the whole card;
suggested sections: Stack, Conventions, Preferences, Current goals."#
    )]
    pub content: Option<String>,
    /// Version the new text was edited from (used by `update_card`).
    #[schemars(
        description = r#"Version of the card your content was edited from (for update_card; 0 when there is no card yet).
Read it with get_card first."#
    )]
    pub expected_version: Option<u64>,
    /// Older versions to return (used by `get_card`).
    #[schemars(
        description = r#"Older versions of the card to return as well (for get_card; default 0)."#
    )]
    pub history: Option<u32>,
}
