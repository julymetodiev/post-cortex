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

## Quick Setup

```bash
pcx setup
```

This interactive wizard:
1. Creates a **session** (or reuses an existing one)
2. Creates a **workspace** (or reuses an existing one)
3. Attaches the session to the workspace
4. Generates project files:
   - `CLAUDE.md` — session IDs + memory rules
   - `.claude/settings.json` — enforcement hooks
   - `.claude/hooks/pcx-compact-reinject.sh` — context recovery after compaction
5. Installs **agent definitions** to `~/.claude/agents/post-cortex-agents/`

Non-interactive mode: `pcx setup --name my-project --non-interactive`

### What Gets Installed

**Project files** (in current directory):

| File | Purpose |
|------|---------|
| `CLAUDE.md` | Session/workspace IDs, memory rules, tool reference |
| `.claude/settings.json` | Three hooks: UserPromptSubmit, Stop, SessionStart |
| `.claude/hooks/pcx-compact-reinject.sh` | Re-injects PCX context after compaction |

**Agent definitions** (in `~/.claude/agents/post-cortex-agents/`):

| Agent | subagent_type | Model | Purpose |
|-------|---------------|-------|---------|
| Context Builder | `context-builder` | Haiku | Log decisions, Q&A, problems, code changes |
| Search Specialist | `search-specialist` | Sonnet | Find past knowledge, semantic search |
| Knowledge Analyst | `knowledge-analyst` | Opus | Summaries, analysis, entity mapping |
| Memory Curator | `memory-curator` | Haiku | Session/workspace management |

> **Note:** For simple search/log operations, call MCP tools directly — it's faster than routing through subagents.

### Hooks

Three hooks enforce the memory rules automatically:

| Hook | Event | Type | Purpose |
|------|-------|------|---------|
| **PCX Reminder** | `UserPromptSubmit` | `command` | Injects session ID and rules before every prompt |
| **Stop Check** | `Stop` | `prompt` | JSON compliance checker; forces continuation if rules violated |
| **Compact Reinject** | `SessionStart` (compact) | `command` | Re-injects PCX context after context window compaction |

## The Workflow

```
User asks question
       │
       ▼
┌─────────────────┐
│ 1. SEARCH       │  ← semantic_search (session → workspace → global)
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
│ 4. LOG          │  ← update_conversation_context
│    discovery    │     (Stop hook verifies this happened)
└─────────────────┘
```

## Knowledge Types

| Type | When to Use | Example |
|------|-------------|---------|
| `qa` | Answered a question | "How does auth work?" → "Uses JWT with refresh tokens" |
| `decision_made` | Made architectural choice | "Chose PostgreSQL for better JSON support" |
| `problem_solved` | Fixed a bug | "Memory leak was due to unclosed connections" |
| `code_change` | Significant code change | "Refactored auth module to use middleware pattern" |
| `requirement_added` | New requirement or constraint | "API must support cursor-based pagination" |
| `concept_defined` | Technical concept explained | "Event sourcing: storing state changes as immutable events" |

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

### Option B: Shared Sessions Across Projects

```
Workspace: frontend-app
├── frontend-auth (role: primary)
└── shared-api-contracts (role: shared)  ← Same session

Workspace: backend-api
├── backend-auth (role: primary)
└── shared-api-contracts (role: shared)  ← Same session
```

### Search Scope

| Scope | Searches | Use When |
|-------|----------|----------|
| `session` | Current task only | Narrow, most relevant |
| `workspace` | Entire project | Cross-session context |
| `global` | All projects | Last resort, broadest |

Always search in order: **session → workspace → global**.

## Recency Bias

Use `recency_bias` to prioritize recent content:

| Scenario | Recommended value |
|----------|-------------------|
| Architecture docs (timeless) | `0.0` (default) |
| Finding recent solutions | `0.3 - 0.7` |
| Debugging recent issues | `0.5 - 1.0` |
| Current sprint context | `1.0+` |

Date filtering: `semantic_search(query: "...", date_from: "2024-06-01", date_to: "2024-12-31")`

## Troubleshooting

### Agent doesn't use memory tools

1. Verify `CLAUDE.md` has rules (re-run `pcx setup` if missing)
2. Check `.claude/settings.json` has hooks — the Stop hook forces compliance
3. The `UserPromptSubmit` hook should inject a `[PCX]` reminder every turn

### Memory seems mixed across projects

1. Verify session is linked to correct workspace
2. Use `scope: "workspace"` instead of `scope: "global"`
3. Check `manage_workspace(action: "get")` to see linked sessions

### Old information appears in results

Use `recency_bias: 0.5` or filter by date: `date_from: "2024-01-01"`
