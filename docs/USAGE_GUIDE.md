# Usage guide

How an agent uses post-cortex well: where its memory goes, what to record, how to search.

## Mental Model

```
Workspace = Project (e.g., "E-commerce Platform", "Mobile App")
    └── Session = Feature/Task (e.g., "Authentication", "API Design")
            └── Memory Entry = Knowledge captured during work
```

**Key insight:** Memory stays isolated per workspace. When searching within a workspace, you only get results from that project's sessions.

> **Why "Workspace" and not "Project"?** post-cortex uses "workspace" because a single project may have multiple workspaces (e.g., `frontend-app`, `backend-api`), and a workspace can span multiple repos. Think of it as a *logical grouping* of related sessions, not a 1:1 project mapping.

## Quick Setup

```bash
pcx setup             # every project: the Claude Code plugin, user-level files elsewhere
pcx setup --project   # this project, with fixed ids
```

`pcx setup` is global: for Claude Code it prints the `/plugin` commands that
install the post-cortex plugin, whose hooks find each project's session and
workspace by themselves (see [Global and project setup](DAEMON_MODE.md#global-and-project-setup)).

`pcx setup --project` is the interactive project wizard:
1. Creates a **session** (or reuses an existing one)
2. Creates a **workspace** (or reuses an existing one)
3. Attaches the session to the workspace
4. Pins their ids in `.post-cortex.json` at the project root, which the
   plugin's hooks (and other agents and teammates) read first
5. For Claude Code, writes nothing else: the plugin carries the MCP server,
   the skill and the hooks. It takes out what setups before the plugin wrote:
   the PCX section of `CLAUDE.md`, post-cortex's hooks in
   `.claude/settings.json` and their scripts in `.claude/hooks/`, and
   `~/.claude/agents/post-cortex-agents/` and `~/.claude/skills/post-cortex-memory/`

Non-interactive mode: `pcx setup --project --name my-project --non-interactive`

### Hooks

The plugin's hooks enforce the memory rules:

| Hook | Event | Purpose |
|------|-------|---------|
| **Banner** | `UserPromptSubmit` | Injects the session and workspace ids and the rules before every prompt |
| **Cards** | `SessionStart` | Prints the user card and the project card |
| **Compact Reinject** | `SessionStart` (compact) | Re-injects the PCX context after compaction |
| **File memory** | `PostToolUse` (reads), `PreToolUse` (edits) | What memory knows of a file, once a session: with its first read, else its first edit is held once with it as the reason and goes through when sent again |
| **Record reminder** | `PostToolUse` (edits) | Past 8 files edited and not recorded, each new one reminds to record them |
| **Stop Check** | `Stop` | Blocks a turn that edited a file no later record names, or ran a command or was opened by a task ending with nothing recorded after |

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

Writing a record costs no model call: it is stored and embedded at once. A
record names the entities it is about (`entities`) and how they relate
(`relations`: `depends_on`, `implements`, `caused_by`, `leads_to`,
`related_to`, `required_by`, `conflicts_with`, `solves`); search follows that
graph to what a hit touches.

### Current and ended

Facts change, history stays. A record ends when a later one supersedes it
(`supersedes` on write, or the same fact restated about the same entities),
when a later one says the opposite (`conflicts_with`), or when it is forgotten
(`manage_entity` `forget`). An ended record is never deleted: it keeps the
reason it ended and ranks lower in search.

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

### Search modes

`mode` picks the reranker: `fast` (none, ~40 ms), `local` (a local
cross-encoder, ~1 s; the cloud has none) or `accurate` (Voyage where every
workspace searched allows it, else local, if any). A search that asked for
a reranker and got none says why (`rerank_skipped`: `no_local_reranker`,
`no_reranker`, `remote_not_allowed`, or in the cloud `plan_limit`: the plan
has no reranker calls left). Without
`mode` the daemon's `search_mode` applies, else the best the configured
rerankers allow. Measured quality per mode:
[Search quality](DAEMON_MODE.md#search-quality).

`sources: "messages"` (or `"all"`) searches the captured conversation too;
`author` keeps one writer's records and `prefer_author` ranks them first.

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

1. Verify the post-cortex plugin is installed and enabled (`/plugin`): it carries the hooks
2. The `UserPromptSubmit` hook injects a `[PCX]` line with the session and the rules every turn; without it the plugin does not reach its backend (check its `url` and key)
3. The `Stop` hook blocks a turn that changed something with nothing recorded after

### Memory seems mixed across projects

1. Verify session is linked to correct workspace
2. Use `scope: "workspace"` instead of `scope: "global"`
3. Check `manage_workspace(action: "get")` to see linked sessions

### Old information appears in results

Use `recency_bias: 0.5` or filter by date: `date_from: "2024-01-01"`
