# CLAUDE.md

**Docs:** [PROJECT.md](PROJECT.md) | **No "Co-Authored-By" in commits**

## Session

**Session ID**  `bf52f62e-8e26-4e9e-8501-c42753d9a9ee`
**Workspace ID** | `ba76f569-a50d-46cd-a5dd-7c5577f12d1f`

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
| `semantic_search` | Before answering code/arch questions |
| `update_conversation_context` | After decisions, fixes, changes |
| `get_structured_summary` | Session analysis and reviews |
| `query_conversation_context` | Entity relationships, keyword search |
| `session` | Create/list sessions |
| `manage_workspace` | Workspace CRUD |

## Hooks

| Event | Purpose |
|-------|---------|
| `UserPromptSubmit` | Injects session ID + rules |
| `Stop` (prompt) | Verifies rule compliance |
| `SessionStart` (compact) | Re-injects PCX context after compaction |
