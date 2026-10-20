---
name: post-cortex-memory
description: Long-term project memory through the post-cortex MCP server (tools named session, semantic_search, update_conversation_context, assemble_context, query_conversation_context, get_structured_summary, manage_workspace, manage_entity, admin). Use it whenever those tools are available and the work could benefit from what was learned before - answering questions about a codebase's architecture or past decisions, resuming work after a break or a context reset, debugging something that may have been fixed before, or checking who depends on a component before changing it. Also use it to record what you learn - decisions, bug root causes, code changes, requirements, definitions - so the next session can find them. Trigger on phrases like "remember this", "what did we decide", "have we seen this before", "log this", "pick up where we left off", "post-cortex", or "PCX", even when the user does not name the tools.
---

# post-cortex memory

post-cortex keeps a durable, searchable memory of a project: every entry is a typed record (a decision, a fixed problem, a code change, ...) linked into an entity graph, and every entry is embedded for semantic search. It only pays off if you **read before you answer** and **write after you learn**. Memory that is never searched is dead weight; memory that is never written is empty.

## 1. Find the session first

Every read and write is scoped to a `session_id` (a UUID). Sessions are usually grouped in a workspace (`workspace_id`) so related repos or sub-projects can be searched together. Resolve them once at the start and reuse them:

1. Look in the project's `CLAUDE.md` / `AGENTS.md` (or the injected context from hooks) for a session ID and workspace ID. `pcx setup` writes them there.
2. Otherwise `session` with `action: "search"` and the project or repo name as `query`, or `action: "list"` to see all.
3. Only if nothing fits, create one: `session` `action: "create"` with a descriptive `name` and `description`, then `manage_workspace` `action: "create"` (or `list` to reuse one) and `add_session` with `role: "primary"`. Tell the user the new IDs so they can be written into the project instructions — otherwise the next session cannot find them.

Do not invent IDs or reuse IDs from another project: writes go to the wrong place silently.

## 2. Read before answering

Search when the answer might already exist: architecture and "why is it like this" questions, anything about past decisions, a bug that looks familiar, a task the user describes as continuing earlier work. A cheap search beats re-deriving (or contradicting) a decision that is already on record.

Narrow-to-wide is the default because the closest scope has the most relevant hits:

```
semantic_search(query, scope="session",   scope_id=<session_id>)
  → too few / weak hits → scope="workspace", scope_id=<workspace_id>
  → still nothing       → scope="global"
```

Write queries as natural-language questions or the key terms of what you need ("why did we switch the embedding model", "SurrealDB schema migration"), not single words. Useful knobs:

- `mode`: `fast` (~40 ms) for quick lookups, `accurate` when missing the answer is costly; the server's default otherwise.
- `sources`: `records` (default) searches the logged records; `messages` searches the raw conversation turns that client hooks capture (when capture is on), for things said but never logged; `all` both.
- `recency_bias`: `0` for timeless architecture, `0.3–0.7` for "what is the current state", `1.0` for "what just broke".
- `interaction_type`: e.g. `["decision_made"]` or `["problem_solved"]` to filter.
- `date_from` / `date_to` (ISO 8601) for time windows.
- `select` / `select_map` (JSONPath, e.g. `$.results[*].text_content`) to keep the payload small — results are verbose, and trimming them saves your context.

Results carry `similarity_score`, `content_type`, `timestamp` and `session_id`; a record returned for context next to a hit has `neighbour_of` set to that hit's `content_id`. Check timestamps before trusting an old entry, and say when you are relying on memory rather than on the code. If memory and the code disagree, the code wins; consider logging the correction.

Other read tools, by question:

| You want to know… | Use |
|---|---|
| "What depends on X / what breaks if I change X?" | `assemble_context` with a query naming X — returns the one-hop entity neighbourhood plus **impact analysis (reverse dependencies)** and an LLM-ready `formatted_text`. It matches entity names, not meaning, so name the components explicitly. |
| "Catch me up on this project" / resuming after a break | `get_structured_summary` (`include: ["decisions","insights","entities","questions"]`, `compact: true`) |
| Decisions, open questions, important entities, statistics, recent changes, references to a file | `query_conversation_context` with the matching `query_type` (see the reference) |
| Exact keyword lookup when semantics miss | `query_conversation_context` `query_type: "search_updates"`, `parameters: {"query": "..."}` |

Use `semantic_search` for meaning ("how do we handle auth"), `assemble_context` for structure ("who uses the storage actor"). They complement each other.

## 3. Write after learning

Log when something worth knowing next week happened: a decision and its rationale, the root cause and fix of a bug, a non-obvious code change, a new requirement or constraint, a definition of a domain concept, or a Q&A that took real effort to answer. Log at the moment of discovery — details are lost at the end of a long session or after a context reset.

Do not log chit-chat, things obvious from the code or git history, or secrets. post-cortex redacts credentials and personal data on write, but treat that as a safety net, not permission.

### Shape of a write

`update_conversation_context` needs: `session_id`, `interaction_type`, `content`, and **at least one entity and one relation**. The graph is what makes later `assemble_context` and impact analysis work, so the server rejects writes without them.

```json
{
  "session_id": "<session_id>",
  "interaction_type": "problem_solved",
  "content": {
    "problem": "Session list returned only 'Found 5 workspaces' with no data",
    "solution": "Workspace/session handlers now include result.data via mcp_result_to_call_result",
    "file": "crates/daemon/src/mcp_service/workspace.rs"
  },
  "entities": [
    {"name": "manage_workspace", "entity_type": "code_component"},
    {"name": "missing result data", "entity_type": "problem"}
  ],
  "relations": [
    {"from_entity": "missing result data", "to_entity": "manage_workspace",
     "relation_type": "caused_by", "context": "handler dropped result.data"}
  ],
  "code_reference": "crates/daemon/src/mcp_service/workspace.rs:68"
}
```

- `interaction_type`: `qa`, `decision_made`, `problem_solved`, `code_change`, `requirement_added`, `concept_defined`.
- `content`: string values only (stringify nested JSON). Typed keys read best later: `decision` + `rationale`, `problem` + `solution`, `question` + `answer`, `concept` + `definition`, `file` + `changes`, `requirement`; or a generic `title` + `description`. Make it self-contained — whoever reads it will not have this conversation.
- `entity_type`: `technology`, `concept`, `problem`, `solution`, `decision`, `code_component`.
- `relation_type`: `depends_on`, `implements`, `caused_by`, `solves`, `leads_to`, `related_to`, `required_by`, `conflicts_with`. Both ends must be listed in `entities`; self-relations are rejected.
- Reuse existing entity names exactly (search or `query_conversation_context` `all_entities` first) — `StorageActor` and `storage actor` become two different nodes.
- Several records at once: pass `updates: [ {interaction_type, content, entities, relations}, ... ]` instead of the single fields. Unsure about the shape? `dry_run: true` validates without writing.
- A success reply is just "Context updated successfully" (bulk: a count). It does not return the new entry's ID; if you later need it (to correct or delete the entry), find it with `semantic_search` — its `content_id` is the `entry_id`.

## 4. Keep the project card current

The project card is a short markdown note (≤ 2 KB) that every session starts with: the stack, the conventions, the user's preferences for this project and the current goals. The user card holds preferences that apply everywhere. Both arrive at session start (the SessionStart hook prints them) and at the top of `assemble_context`.

When you learn something that belongs there — a convention the user insists on, a goal that changed, a stack decision — update it: `manage_workspace` `action: "get_card"` with the `workspace_id`, edit the text, then `action: "update_card"` with the whole new `content` and the `expected_version` you read (0 if there was none). If the reply says the card changed meanwhile, merge into the version it returns and retry. Keep it short and current: replace stale goals instead of appending; details belong in records, not the card. Use `card: "user"` for the user card, and only for preferences the user stated.

## 5. Housekeeping

- Wrong or outdated entry: `manage_entity` `action: "delete_update"` with its `entry_id` (the `content_id` from search), then write the corrected version. `action: "delete"` with `entity_name` removes an entity and its edges.
- Rename / re-describe a session: `session` `action: "update_metadata"`.
- Search returns nothing for content you know exists: `admin` `action: "vectorize_session"` (then `vectorize_stats`). `admin` `action: "health"` checks the daemon.
- Deletes (`session` `delete`, `manage_workspace` `delete`, `manage_entity`) are permanent — confirm with the user before running them.

## 6. When things fail

| Symptom | Likely cause → fix |
|---|---|
| Tools missing entirely | post-cortex is not connected; see `references/setup.md` |
| `Session not found` | wrong/stale ID → `session` `search` / `list` |
| Write rejected: entities / relations | add ≥1 entity and ≥1 relation whose ends are both in `entities` |
| HTTP 401 | daemon has `auth_token`; the client must send `Authorization: Bearer <token>` |
| HTTP 403 "Host header is not allowed" | daemon on `0.0.0.0` without that hostname in `allowed_hosts` |
| Empty search on a populated session | widen scope, rephrase, lower `recency_bias`, or `vectorize_session` |

Don't let a memory failure block the user's actual task: say what failed, carry on with the work, and retry the write later.

## Reference

`references/tools.md` — every tool's parameters, all `query_conversation_context` query types with their parameters, and response fields. Read it when you need a parameter this page does not show.
`references/setup.md` — installing and connecting post-cortex (Claude Code, other MCP clients, daemon vs stdio, remote daemons).
