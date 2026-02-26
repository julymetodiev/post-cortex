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

Create a `CLAUDE.md` file in your project root:

```markdown
# CLAUDE.md

## Session Context

| Key | Value |
|-----|-------|
| **Session ID** | `your-session-uuid-here` |
| **Workspace ID** | `your-workspace-uuid-here` |
| **Project** | my-project |

## Mandatory Rules

### RULE 1: Search Before Answering
Before answering questions about code or architecture, call `semantic_search` directly.

### RULE 2: Log After Discovery
After discovering anything new or making decisions, call `update_conversation_context` directly.

### RULE 3: Self-Check (enforced by Stop hook)
The Stop hook automatically verifies Rule 1 & 2 compliance after every response.

## MCP Tools (Direct Calls)

Use these tools **directly** — do NOT use subagents for search/log operations.

| Tool | Purpose | When |
|------|---------|------|
| `semantic_search` | Search knowledge | **Before** answering code/arch questions |
| `update_conversation_context` | Log discoveries | **After** decisions, fixes, changes |
| `get_structured_summary` | Session summaries | For analysis and reviews |
| `query_conversation_context` | Entity queries | For entity relationships |
| `session` | Session CRUD | Create/list sessions |
| `manage_workspace` | Workspace CRUD | Manage workspaces |

## Subagents (Complex Analysis Only)

| Agent | subagent_type | When to Use |
|-------|---------------|-------------|
| Analyst | `knowledge-analyst` | Multi-step summaries, cross-session analysis |
| Curator | `memory-curator` | Complex workspace reorganization |

> **Do NOT use** `search-specialist` or `context-builder` subagents. Call MCP tools directly instead.
```

See [CLAUDE.md](../CLAUDE.md) for a complete working example.

### 3. Configure Hooks

Hooks enforce the mandatory rules automatically via Claude Code's hook system. Add them to `.claude/settings.json`:

```json
{
  "hooks": {
    "UserPromptSubmit": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "echo '[PCX] Session: <session-id> | Rules: 1) Search PCX before answering code/arch questions 2) Log discoveries to PCX after'"
          }
        ]
      }
    ],
    "Stop": [
      {
        "hooks": [
          {
            "type": "prompt",
            "prompt": "Check if Claude followed PCX rules: 1) Called semantic_search before answering code/arch questions 2) Called update_conversation_context after making changes. If rules were followed or not applicable, respond {\"ok\": true}. If violated, respond {\"ok\": false, \"reason\": \"PCX Rule violated: [which rule]. Session ID: <session-id>\"}."
          }
        ]
      }
    ],
    "SessionStart": [
      {
        "matcher": "compact",
        "hooks": [
          {
            "type": "command",
            "command": "\"$CLAUDE_PROJECT_DIR\"/.claude/hooks/pcx-compact-reinject.sh"
          }
        ]
      }
    ]
  }
}
```

| Hook | Event | Type | Purpose |
|------|-------|------|---------|
| **PCX Reminder** | `UserPromptSubmit` | `command` | Injects session ID and rules before every prompt |
| **Stop Check** | `Stop` | `prompt` | Verifies Rule 1 & 2 compliance; forces continuation if violated |
| **Compact Reinject** | `SessionStart` (compact) | `command` | Re-injects PCX context after context window compaction |

**How hooks work:**

- **`UserPromptSubmit`** (command): Runs before every user message. The echo output is injected as a system reminder, ensuring Claude always knows the session ID and rules.
- **`Stop`** (prompt): A prompt-type hook that runs when Claude is about to stop responding. A small model evaluates whether the rules were followed. If `{\"ok\": false}`, Claude is forced to continue and fulfill the missing rule.
- **`SessionStart`** with `matcher: "compact"`: Runs when the context window is compacted (messages compressed). The shell script re-injects session context and optionally fetches a recent summary from the PCX daemon.

**Compact reinject script** (`.claude/hooks/pcx-compact-reinject.sh`):

The script outputs static PCX context (session ID, workspace ID, rules) and optionally fetches a recent summary from the running PCX daemon via HTTP. This ensures context survives compaction. See [pcx-compact-reinject.sh](../.claude/hooks/pcx-compact-reinject.sh) for the full implementation.

### 4. (Optional) Add Agent Definitions

For advanced multi-step workflows, add custom agents in `.claude/agents/`:

```
.claude/
├── hooks/
│   └── pcx-compact-reinject.sh  # Compact reinject script
├── settings.json                # Hook configuration
└── agents/
    └── post-cortex-agents/
        ├── SKILL.md             # Main skill definition
        └── agents/
            ├── knowledge-analyst.md  # Summaries and analysis
            └── memory-curator.md     # Session/workspace management
```

> **Note:** The `search-specialist` and `context-builder` agents still exist for backwards compatibility but are **deprecated**. Call MCP tools directly instead — it's faster and avoids unnecessary subagent overhead.

See [.claude/agents/](../.claude/agents/) for working examples.

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
| [CLAUDE.md](../CLAUDE.md) | Working example of project configuration |
| [.claude/settings.json](../.claude/settings.json) | Hook configuration (enforcement) |
| [.claude/hooks/](../.claude/hooks/) | Hook scripts (compact reinject) |
| [.claude/agents/](../.claude/agents/) | Custom agent definitions |
| [PROJECT.md](../PROJECT.md) | Development documentation |
