# CLAUDE.md

Project instructions for AI assistants working with Post-Cortex.

**Development docs:** [PROJECT.md](PROJECT.md) | **No "Co-Authored-By" in commits**

---

## Session Context

| Key | Value |
|-----|-------|
| **Session ID** | `bf52f62e-8e26-4e9e-8501-c42753d9a9ee` |
| **Workspace ID** | `c7c6dfa7-85c6-42b0-9b37-f2034c569a71` |
| **Project** | post-cortex |

> **Note:** These IDs are project-specific. Replace with your own when adapting this file for a different project. See [USAGE_GUIDE.md](docs/USAGE_GUIDE.md#1-create-session-and-workspace) for setup instructions.

---

## Mandatory Rules

### RULE 1: Search Before Answering

**BEFORE answering ANY question about code, architecture, or past decisions:**

1. Call `semantic_search` directly (session → workspace → global)
2. Check results
3. Then formulate answer

```
mcp__post-cortex__semantic_search(
  query: "your question",
  scope: "session",
  scope_id: "bf52f62e-8e26-4e9e-8501-c42753d9a9ee"
)
```

**NO EXCEPTIONS.**

### RULE 2: Log After Discovery

**AFTER discovering anything new, making decisions, or changing code:**

1. Call `update_conversation_context` directly
2. Log what you discovered/decided/solved
3. Can run in background to not block response

```
mcp__post-cortex__update_conversation_context(
  session_id: "bf52f62e-8e26-4e9e-8501-c42753d9a9ee",
  interaction_type: "decision_made",
  content: { "decision": "...", "rationale": "..." }
)
```

| Situation | Type |
|-----------|------|
| Answered a question | `qa` |
| Made a decision | `decision_made` |
| Fixed a bug | `problem_solved` |
| Changed code | `code_change` |
| New requirement | `requirement_added` |
| Concept explained | `concept_defined` |

### RULE 3: Self-Check (enforced by Stop hook)

The `Stop` hook automatically verifies after EVERY response:
- Did I search before answering a codebase question?
- Did I log any new discoveries?

If not → Claude is forced to continue and fulfill the missing rule.

---

## MCP Tools (Direct Calls)

Use these tools **directly** — do NOT use subagents for search/log operations.

| Tool | Purpose | When |
|------|---------|------|
| `semantic_search` | Search knowledge | **Before** answering code/arch questions |
| `update_conversation_context` | Log discoveries | **After** decisions, fixes, changes |
| `get_structured_summary` | Session summaries | For analysis and reviews |
| `query_conversation_context` | Entity queries | For entity relationships, keyword search |
| `session` | Session CRUD | Create/list sessions |
| `manage_workspace` | Workspace CRUD | Manage workspaces |

### Search Examples

**Session search:**
```
semantic_search(query: "...", scope: "session", scope_id: "bf52f62e-...")
```

**Workspace search (cross-session):**
```
semantic_search(query: "...", scope: "workspace", scope_id: "c7c6dfa7-...")
```

**Global search:**
```
semantic_search(query: "...", scope: "global")
```

### Log Examples

**Log decision:**
```json
update_conversation_context(
  session_id: "bf52f62e-...",
  interaction_type: "decision_made",
  content: { "decision": "Use X", "rationale": "Because Y", "alternatives": "Z" }
)
```

**Log Q&A:**
```json
update_conversation_context(
  session_id: "bf52f62e-...",
  interaction_type: "qa",
  content: { "question": "How does X work?", "answer": "X works by..." }
)
```

**Bulk log:**
```json
update_conversation_context(
  session_id: "bf52f62e-...",
  updates: [
    { interaction_type: "qa", content: { "question": "...", "answer": "..." } },
    { interaction_type: "decision_made", content: { "decision": "...", "rationale": "..." } }
  ]
)
```

> **Tip:** Use `recency_bias` for time-sensitive searches (e.g., recent bugs). See [USAGE_GUIDE.md](docs/USAGE_GUIDE.md#handling-knowledge-obsolescence) for recommended values.

---

## Subagents (Complex Analysis Only)

Use subagents **only** for multi-step analysis that requires multiple tool calls:

| Agent | subagent_type | When to Use |
|-------|---------------|-------------|
| Analyst | `knowledge-analyst` | Multi-step summaries, cross-session analysis |
| Curator | `memory-curator` | Complex workspace reorganization |

> **Do NOT use** `search-specialist` or `context-builder` subagents. Call MCP tools directly instead.

---

## Hooks (Automatic Enforcement)

| Hook | Event | Purpose |
|------|-------|---------|
| PCX Reminder | `UserPromptSubmit` | Injects session ID + rules before every prompt |
| Stop Check | `Stop` (prompt) | Verifies Rule 1 & 2 compliance before stopping |
| Compact Reinject | `SessionStart` (compact) | Re-injects PCX context after compaction |
