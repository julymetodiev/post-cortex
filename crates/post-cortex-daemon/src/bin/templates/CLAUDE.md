# CLAUDE.md

## Session

| Session ID | `<SESSION_ID>` |
|---|---|
| **Workspace ID** | `<WORKSPACE_ID>` |

## Rules

1. **Search before answering** — call `semantic_search` (session → workspace → global) BEFORE answering any code/architecture question
2. **Log after discovery** — call `update_conversation_context` AFTER making decisions, fixing bugs, or changing code
3. **Self-check** — enforced by Stop hook automatically

### Interaction Types

`qa` · `decision_made` · `problem_solved` · `code_change` · `requirement_added` · `concept_defined`

## MCP Tools

Call directly — do NOT use subagents for simple search/log operations.

| Tool | When |
|------|------|
| `semantic_search` | Before answering code/arch questions. Scope: session / workspace / global. Returns structured payload (scores, content_id, content_type, snippet, timestamp) |
| `update_conversation_context` | After decisions, fixes, changes. Single or bulk. **`entities` (≥1) and `relations` (≥1) are required** (dangling/self-relations rejected). Content: typed keys (decision/rationale, problem/solution, concept/definition, question/answer, file/changes, requirement) or generic `title`/`description` |
| `get_structured_summary` | Session analysis and reviews. `include` values: `decisions`, `insights`, `entities`, `questions`, `all`. Also: `decisions_limit`, `entities_limit`, `questions_limit`, `concepts_limit`, `min_confidence`, `compact` |
| `query_conversation_context` | Flexible queries. `query_type` values: `find_related_entities`, `get_entity_context`, `search_updates`, `entity_importance`, `entity_network`, `find_related_content` (topic, max_results), `key_decisions`, `key_insights` (limit), `session_statistics`, `structured_summary`, `decisions`, `open_questions`, `assemble_context`, `trace_relationships`, `get_most_important_entities`, `find_entities_by_type` |
| `session` | Sessions: `create`, `list`, `load` (session_id), `search` (query), `update_metadata` (session_id + name/description; preserves created_at), `delete` (evicts cache + storage) |
| `manage_workspace` | Workspace CRUD: `create`, `list`, `get`, `delete`, `add_session`, `remove_session` |
| `assemble_context` | Graph-aware retrieval (semantic + 1-hop traversal + **impact analysis = reverse dependencies, "who depends on X"**). Scope: session or workspace. Returns items (budget-packed), entity_context (with `DirectMention` / `GraphNeighbor{via,relation}`), impact, total_tokens, formatted_text (LLM-ready) |
| `manage_entity` | `delete` (entity_name; cascades typed edges), `delete_update` (entry_id; removes a single context row from hot/warm/incremental caches and persists) |
| `admin` | `health`, `vectorize_session` (session_id), `vectorize_stats`, `create_checkpoint` (session_id) |

## Hooks

| Event | Purpose |
|-------|---------|
| `UserPromptSubmit` | Injects session ID + rules |
| `Stop` (prompt) | Verifies rule compliance |
| `SessionStart` (compact) | Re-injects PCX context after compaction |
