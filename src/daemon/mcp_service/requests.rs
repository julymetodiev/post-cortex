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

#[derive(Deserialize, JsonSchema, Debug)]
pub struct AssembleContextRequest {
    #[schemars(description = r#"Session UUID. Required when workspace_id is not provided.

Format: xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx

Note: If both session_id and workspace_id are provided, workspace_id takes precedence
(context is merged across all sessions in the workspace)."#)]
    pub session_id: Option<String>,
    #[schemars(
        description = r#"Workspace UUID. When provided, context is merged across all sessions in the workspace.

Format: xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx

Note: Takes precedence over session_id."#
    )]
    pub workspace_id: Option<String>,
    #[schemars(description = r#"The query/topic to assemble context for.

Example: "How does authentication interact with the user profile service?"

Note: Entity extraction runs on this query to seed graph traversal."#)]
    pub query: String,
    #[schemars(
        description = r#"Token budget for the assembled context (default: 4000).

Examples:
- 2000: tight context window
- 4000: default (matches Axon's per-call budget)
- 8000: extended chains / orchestration

Note: Must be > 0. Items are packed highest-score-first until budget is consumed."#
    )]
    pub token_budget: Option<u32>,
}

// =============================================================================
// Tool 8: manage_entity
// =============================================================================

#[derive(Deserialize, JsonSchema, Debug)]
pub struct ManageEntityRequest {
    #[schemars(description = r#"Action to perform on session content.

Valid values:
- delete: Remove an entity from the session (cascades typed edges). Requires entity_name.
- delete_update: Remove a single context update (ghost / bad row) by entry_id. Requires entry_id.

Note: Must be lowercase. Additional actions may be added later."#)]
    pub action: String,
    #[schemars(description = r#"Session UUID containing the entity/update.

Format: xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx"#)]
    pub session_id: String,
    #[schemars(description = r#"Entity name to operate on (required for action=delete).

Example: "RocksDB"

Note: Names are matched case-insensitively in the entity graph."#)]
    pub entity_name: Option<String>,
    #[schemars(description = r#"Context-update id (required for action=delete_update).

Format: xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx

Note: This is the `entry_id` returned by `assemble_context` items, or the id of a
ContextUpdate stored in the session. Use it to remove ghost / mis-shaped writes."#)]
    pub entry_id: Option<String>,
}

// =============================================================================
// Tool 9: admin
// =============================================================================

#[derive(Deserialize, JsonSchema, Debug)]
pub struct AdminRequest {
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
    #[schemars(description = r#"Session UUID for vectorize_session and create_checkpoint.

Format: xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx"#)]
    pub session_id: Option<String>,
}

// =============================================================================
// Tool 1: session
// =============================================================================

#[derive(Deserialize, JsonSchema, Debug)]
pub struct SessionRequest {
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
    #[schemars(description = r#"Session name (used by create and update_metadata).

Examples:
- "Feature: Authentication"
- "Bug Fix: Memory Leak"

Note: For update_metadata, only provided fields are changed."#)]
    pub name: Option<String>,
    #[schemars(description = r#"Session description (used by create and update_metadata).

Example: "Working on implementing OAuth2 login flow"

Note: For update_metadata, only provided fields are changed."#)]
    pub description: Option<String>,
    #[schemars(description = r#"Session UUID (required for load, update_metadata, delete).

Format: xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx

Example: "60c598e2-d602-4e07-a328-c458006d48c7""#)]
    pub session_id: Option<String>,
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

#[derive(Deserialize, JsonSchema, Debug, Clone)]
pub struct ContextUpdateItem {
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
    #[schemars(
        description = r#"Content as key-value pairs (all values must be strings).

Format: HashMap<String, String> where both keys and values are strings.

Examples:
Simple: {"decision": "Use Rust", "rationale": "Performance"}
Complex: {"criteria": "{\"performance\": 9, \"safety\": 10}", "date": "2025-01-12"}

Note: For complex nested data, stringify as JSON first. Do not pass nested objects directly."#
    )]
    pub content: std::collections::HashMap<String, String>,
    #[schemars(description = r#"Optional code reference for context.

Can be a simple string or complex object:

Examples:
- Simple: "src/main.rs:42"
- Complex: {"file": "src/main.rs", "line": 42, "function": "process_data"}

Note: Helps link context to specific code locations."#)]
    pub code_reference: Option<serde_json::Value>,
}

#[derive(Deserialize, JsonSchema, Debug)]
pub struct UpdateConversationContextRequest {
    #[schemars(description = r#"Session ID (36-char UUID format with hyphens).

Format: xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx

How to get:
1. Create session: Use session tool with action='create'
2. Find session: Use semantic_search to search for previous sessions

Example: "60c598e2-d602-4e07-a328-c458006d48c7"

Note: Must be a string, not a number. UUIDs are always strings."#)]
    pub session_id: String,
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
    #[schemars(
        description = r#"Content as key-value pairs for single update (all values must be strings).

Format: HashMap<String, String> where both keys and values are strings.

Examples:
Simple: {"decision": "Use Rust", "rationale": "Performance"}
Complex: {"criteria": "{\"performance\": 9}", "date": "2025-01-12"}

Note: For complex nested data, stringify as JSON first. Required for single update mode."#
    )]
    pub content: Option<std::collections::HashMap<String, String>>,
    #[schemars(description = r#"Optional code reference for single update.

Can be a simple string or complex object:

Examples:
- Simple: "src/main.rs:42"
- Complex: {"file": "src/main.rs", "line": 42, "function": "process_data"}"#)]
    pub code_reference: Option<serde_json::Value>,
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
}

// =============================================================================
// Tool 3: semantic_search (unified)
// =============================================================================

#[derive(Deserialize, JsonSchema, Debug)]
pub struct SemanticSearchRequest {
    #[schemars(description = r#"Search query for semantic search.

Examples:
- "How did we handle authentication?"
- "What were the performance issues?"
- "decision_made API design"

Note: Natural language queries work best. The search understands context."#)]
    pub query: String,
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
    #[schemars(
        description = r#"Maximum number of results to return (default: 10, max: 100).

Examples:
- 5: Return top 5 most relevant results
- 20: Return top 20 results
- null or omit: Use default of 10

Note: Higher values may slow down search. Maximum allowed is 100."#
    )]
    pub limit: Option<usize>,
    #[schemars(
        description = r#"Filter results from this date onwards (ISO 8601 format).

Format: YYYY-MM-DD or YYYY-MM-DDTHH:MM:SSZ

Examples:
- "2025-01-01": Results from January 1st, 2025 onwards
- "2025-01-15T10:00:00Z": Results from January 15th, 2025 at 10am UTC onwards

Note: Use with date_to to specify a date range."#
    )]
    pub date_from: Option<String>,
    #[schemars(description = r#"Filter results up to this date (ISO 8601 format).

Format: YYYY-MM-DD or YYYY-MM-DDTHH:MM:SSZ

Examples:
- "2025-01-31": Results up to January 31st, 2025
- "2025-01-15T10:00:00Z": Results up to January 15th, 2025 at 10am UTC

Note: Use with date_from to specify a date range."#)]
    pub date_to: Option<String>,
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
}

// =============================================================================
// Tool 4: get_structured_summary (extended)
// =============================================================================

#[derive(Deserialize, JsonSchema, Debug)]
pub struct GetStructuredSummaryRequest {
    #[schemars(description = "Session ID")]
    pub session_id: String,
    #[schemars(
        description = "Sections to include: decisions, insights, entities, all (default: all)"
    )]
    pub include: Option<Vec<String>>,
    #[schemars(description = "Maximum decisions to include")]
    pub decisions_limit: Option<usize>,
    #[schemars(description = "Maximum entities to include")]
    pub entities_limit: Option<usize>,
    #[schemars(description = "Maximum questions to include")]
    pub questions_limit: Option<usize>,
    #[schemars(description = "Maximum concepts to include")]
    pub concepts_limit: Option<usize>,
    #[schemars(description = "Minimum confidence level")]
    pub min_confidence: Option<f32>,
    #[schemars(description = "Use compact format for large sessions")]
    pub compact: Option<bool>,
}

// =============================================================================
// Tool 5: query_conversation_context
// =============================================================================

#[derive(Deserialize, JsonSchema, Debug)]
pub struct QueryConversationContextRequest {
    #[schemars(description = "Session ID")]
    pub session_id: String,
    #[schemars(
        description = "Query type. Supported: find_related_entities, get_entity_context, search_updates, entity_importance, entity_network, find_related_content (topic, max_results), key_decisions, key_insights (limit), session_statistics, structured_summary, decisions, open_questions, related_entities, all_entities, trace_relationships, get_most_important_entities, get_recently_mentioned_entities, analyze_entity_importance, find_entities_by_type, assemble_context, recent_changes, code_references."
    )]
    pub query_type: String,
    #[schemars(description = "Query parameters as key-value pairs")]
    pub parameters: std::collections::HashMap<String, String>,
}

// =============================================================================
// Tool 6: manage_workspace
// =============================================================================

#[derive(Deserialize, JsonSchema, Debug)]
pub struct ManageWorkspaceRequest {
    #[schemars(description = r#"Action to perform on workspaces.

Valid values:
- create: Create a new workspace (requires name and description)
- list: List all workspaces
- get: Get workspace details (requires workspace_id)
- delete: Delete a workspace (requires workspace_id)
- add_session: Add a session to workspace (requires workspace_id, session_id, optional role)
- remove_session: Remove a session from workspace (requires workspace_id, session_id)

Examples:
✅ {"action": "create", "name": "Auth Feature", "description": "OAuth2 work"}
✅ {"action": "get", "workspace_id": "f1d2e3a4-..."}
✅ {"action": "add_session", "workspace_id": "...", "session_id": "...", "role": "primary"}

Note: Must be lowercase. Different actions require different parameters."#)]
    pub action: String,
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
    #[schemars(description = r#"Workspace name (for create action).

Example: "Authentication Feature" or "API Design"

Note: Only used when action='create'. Helps identify the workspace."#)]
    pub name: Option<String>,
    #[schemars(description = r#"Workspace description (for create action).

Example: "Working on OAuth2 authentication and user management"

Note: Only used when action='create'. Provides context for the workspace."#)]
    pub description: Option<String>,
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
}
