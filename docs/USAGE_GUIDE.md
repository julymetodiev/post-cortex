# Post-Cortex Usage Guide

How to effectively use Post-Cortex for AI assistant memory management.

## Mental Model

```
Workspace = Project (e.g., "E-commerce Platform", "Mobile App")
    └── Session = Feature/Task (e.g., "Authentication", "API Design")
            └── Memory Entry = Knowledge captured during work
```

**Key insight:** Memory stays isolated per workspace. When searching within a workspace, you only get results from that project's sessions.

> **Why "Workspace" and not "Project"?** Post-Cortex uses "workspace" because a single project may have multiple workspaces (e.g., `frontend-app`, `backend-api`), and a workspace can span multiple repos. Think of it as a *logical grouping* of related sessions, not a 1:1 project mapping.

## Quick Setup for New Projects

### 1. Create Session and Workspace

```
# Create session for your project
session(action: "create", name: "my-project", description: "Main development session")

# Create workspace to group sessions
manage_workspace(action: "create", name: "my-project", description: "My Project Development")

# Link session to workspace
manage_workspace(action: "add_session", workspace_id: "...", session_id: "...", role: "primary")
```

### 2. Configure CLAUDE.md

Copy the template from `docs/examples/CLAUDE.md` to your project root and replace the placeholder IDs:

```markdown
# CLAUDE.md

## Session

| Session ID | `<YOUR_SESSION_ID>` |
|---|---|
| **Workspace ID** | `<YOUR_WORKSPACE_ID>` |

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
```

See [docs/examples/CLAUDE.md](examples/CLAUDE.md) for the full template.

### 3. Configure Hooks

Copy `docs/examples/settings.json` to `.claude/settings.json` and replace `<YOUR_SESSION_ID>` with your session UUID.

Three hooks enforce the rules automatically:

| Hook | Event | Type | Purpose |
|------|-------|------|---------|
| **PCX Reminder** | `UserPromptSubmit` | `command` | Injects session ID and rules before every prompt |
| **Stop Check** | `Stop` | `prompt` | JSON-only compliance checker; forces continuation if rules violated |
| **Compact Reinject** | `SessionStart` (compact) | `command` | Re-injects PCX context after context window compaction |

**How they work:**

- **`UserPromptSubmit`**: Echo output is injected as a system reminder, ensuring Claude always knows the session ID and rules.
- **`Stop`**: A prompt-type hook where a small model checks rule compliance. Outputs `{"ok": true}` or `{"ok": false, "reason": "..."}`. If false, Claude is forced to continue.
- **`SessionStart`** with `matcher: "compact"`: Runs the reinject script when context is compacted. Copy `docs/examples/pcx-compact-reinject.sh` to `.claude/hooks/` and replace the placeholder IDs.

See [docs/examples/settings.json](examples/settings.json) and [docs/examples/pcx-compact-reinject.sh](examples/pcx-compact-reinject.sh) for templates.

### 4. (Optional) Add Agent Definitions

For advanced multi-step workflows, copy agents to `~/.claude/agents/` (global) or `.claude/agents/` (per-project):

```
cp -r docs/examples/agents ~/.claude/agents/post-cortex-agents
```

This adds 4 specialized subagents:

| Agent | subagent_type | Model | Purpose |
|-------|---------------|-------|---------|
| Context Builder | `context-builder` | Haiku | Log decisions, Q&A, problems, code changes |
| Search Specialist | `search-specialist` | Sonnet | Find past knowledge, semantic search |
| Knowledge Analyst | `knowledge-analyst` | Opus | Summaries, analysis, entity mapping |
| Memory Curator | `memory-curator` | Haiku | Session/workspace management |

> **Note:** For simple search/log operations, call MCP tools directly — it's faster than routing through subagents.

See [docs/examples/agents/](examples/agents/) for all agent definitions.

## The Workflow

```
User asks question
       │
       ▼
┌─────────────────┐
│ 1. SEARCH       │  ← Call semantic_search directly (session → workspace → global)
│    past memory  │
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ 2. EXPLORE      │  ← If not found, explore codebase
│    codebase     │
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ 3. RESPOND      │  ← Answer user
│    to user      │
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ 4. LOG          │  ← Call update_conversation_context directly
│    discovery    │     (Stop hook verifies this happened)
└─────────────────┘
```

## Multi-Project Setup

### Option A: One Workspace per Project

```
Workspace: frontend-app
├── frontend-auth (role: primary)
├── frontend-ui-components (role: related)
└── frontend-api-client (role: dependency)

Workspace: backend-api
├── backend-auth (role: primary)
├── backend-database (role: related)
└── backend-messaging (role: dependency)
```

**Search scope:**
- `scope: "session"` → Current task only
- `scope: "workspace"` → Entire project
- `scope: "global"` → All projects

#### Canonical Search Pattern

When searching for knowledge, always follow this narrowing order:

```
1. session  → Search current task first (fastest, most relevant)
2. workspace → Search entire project (cross-session context)
3. global   → Search all projects (last resort, broadest)
```

This `session → workspace → global` pattern is used consistently across all agents and error handling.

### Option B: Shared Sessions Across Projects

```
Workspace: frontend-app
├── frontend-auth (role: primary)
└── shared-api-contracts (role: shared)  ← Same session

Workspace: backend-api
├── backend-auth (role: primary)
└── shared-api-contracts (role: shared)  ← Same session
```

Use `role: "shared"` for sessions that belong to multiple workspaces.

## Knowledge Types

Use the right type when logging:

| Type | When to Use | Example |
|------|-------------|---------|
| `qa` | Answered a question | "How does auth work?" → "Uses JWT with refresh tokens" |
| `decision_made` | Made architectural choice | "Chose PostgreSQL for better JSON support" |
| `problem_solved` | Fixed a bug | "Memory leak was due to unclosed connections" |
| `code_change` | Significant code change | "Refactored auth module to use middleware pattern" |
| `requirement_added` | New requirement or constraint | "API must support pagination with cursor-based tokens" |
| `concept_defined` | Technical concept explained | "Event sourcing: storing state changes as immutable events" |

## Handling Knowledge Obsolescence

Old knowledge can become stale. Use `recency_bias` to prioritize recent content:

```
# Recent bugs/issues - prioritize fresh content
semantic_search(query: "timeout errors", recency_bias: 0.7)

# Architecture decisions - all time equally relevant
semantic_search(query: "database choice", recency_bias: 0.0)

# Date-filtered search
semantic_search(
  query: "API changes",
  date_from: "2024-06-01",
  date_to: "2024-12-31"
)
```

| Scenario | Recommended `recency_bias` |
|----------|---------------------------|
| Debugging recent issues | 0.5 - 1.0 |
| Finding latest solutions | 0.3 - 0.7 |
| Architecture docs (timeless) | 0.0 |
| Current sprint context | 1.0+ |

## Common Patterns

### Pattern 1: Project Onboarding

When starting work on an existing project:

```
1. semantic_search(query: "project architecture overview", scope: "workspace")
2. get_structured_summary(session_id: "...", include: ["decisions"])
3. query_conversation_context(query_type: "entity_importance")
```

### Pattern 2: Before Making Changes

```
1. Search for related past decisions
2. Check if similar problems were solved before
3. Review relevant code change history
```

### Pattern 3: End of Session

```
1. Review what was discovered/decided
2. Ensure all important context was logged
3. Update session description if scope changed
```

## Troubleshooting

### Agent doesn't use memory tools

1. **Add explicit rules** in CLAUDE.md (see template above)
2. **Add hooks** in `.claude/settings.json` — the Stop hook will force compliance:
```markdown
**RULE 1: Search Before Answering**
MUST call semantic_search directly before answering ANY question about code.
**NO EXCEPTIONS.**
```
3. **Verify hooks are active:** The `UserPromptSubmit` hook should inject a `[PCX]` reminder in every conversation turn

### Memory seems mixed across projects

Check workspace isolation:
1. Verify session is linked to correct workspace
2. Use `scope: "workspace"` instead of `scope: "global"`
3. Check `manage_workspace(action: "get")` to see linked sessions

### Old information appears in results

Use recency bias:
```
semantic_search(query: "...", recency_bias: 0.5)
```

Or filter by date:
```
semantic_search(query: "...", date_from: "2024-01-01")
```

## Reference Files

| File | Purpose |
|------|---------|
| [docs/examples/CLAUDE.md](examples/CLAUDE.md) | CLAUDE.md template with placeholders |
| [docs/examples/settings.json](examples/settings.json) | Hook configuration template |
| [docs/examples/pcx-compact-reinject.sh](examples/pcx-compact-reinject.sh) | Compact reinject script template |
| [docs/examples/agents/](examples/agents/) | Agent definitions (copy to `~/.claude/agents/`) |
| [PROJECT.md](../PROJECT.md) | Development documentation |
