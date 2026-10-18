# post-cortex MCP tool reference (0.4)

Nine tools. IDs are UUID strings. Optional fields can be omitted.

## Contents
- [session](#session)
- [manage_workspace](#manage_workspace)
- [semantic_search](#semantic_search)
- [update_conversation_context](#update_conversation_context)
- [assemble_context](#assemble_context)
- [query_conversation_context](#query_conversation_context)
- [get_structured_summary](#get_structured_summary)
- [manage_entity](#manage_entity)
- [admin](#admin)
- [Response shape](#response-shape)

## session

`action` (required): `create` | `list` | `load` | `search` | `update_metadata` | `delete`

| Action | Fields |
|---|---|
| `create` | `name`, `description` (both optional but strongly recommended) |
| `list` | — (reads lightweight summaries; does not load sessions) |
| `load` | `session_id` |
| `search` | `query` (matches name / description) |
| `update_metadata` | `session_id`, `name` and/or `description` (keeps `created_at`) |
| `delete` | `session_id` — permanent: updates, graph and embeddings |

## manage_workspace

`action` (required): `create` | `list` | `get` | `delete` | `add_session` | `remove_session`

| Action | Fields |
|---|---|
| `create` | `name`, `description` |
| `list` | — |
| `get` | `workspace_id` |
| `delete` | `workspace_id` (sessions are kept) |
| `add_session` | `workspace_id`, `session_id`, `role`: `primary` \| `related` \| `dependency` \| `shared` |
| `remove_session` | `workspace_id`, `session_id` |

## semantic_search

| Field | Notes |
|---|---|
| `query` (required) | natural language |
| `scope` | `session` \| `workspace` \| `global` (default) |
| `scope_id` | session or workspace UUID; required for those scopes |
| `limit` | default 10, max 100 |
| `recency_bias` | `0` (off, default) … `1.0+`; score × e^(−λ·days/365) |
| `interaction_type` | array filter, e.g. `["decision_made","problem_solved"]` |
| `date_from`, `date_to` | `YYYY-MM-DD` or RFC 3339 |
| `select` | one JSONPath, e.g. `$.results[0:3].text_content` |
| `select_map` | `{key: JSONPath}`, e.g. `{"texts":"$.results[*].text_content","ids":"$.results[*].content_id"}` |

JSONPath subset: `$`, `.field`, `[n]`, `[-n]`, `[a:b]`, `[*]`, `.*`, `[0,3,7]`. No `..`, filters or functions.

Result fields (all scopes): `content_id`, `session_id`, `content_type`, `text_content`, `similarity_score`, `importance_score`, `combined_score`, `timestamp`.

## update_conversation_context

Single mode: `session_id`, `interaction_type`, `content`, `entities`, `relations`, optional `code_reference`, optional `dry_run`.
Bulk mode: `session_id`, `updates: [{interaction_type, content, entities, relations, code_reference?}]`.

- `interaction_type`: `qa`, `decision_made`, `problem_solved`, `code_change`, `requirement_added`, `concept_defined`
- `content`: object of string → string
- `entities[]`: `{name, entity_type}`; `entity_type` ∈ `technology`, `concept`, `problem`, `solution`, `decision`, `code_component` (unknown → `concept`)
- `relations[]`: `{from_entity, to_entity, relation_type, context}`; `relation_type` ∈ `depends_on`, `implements`, `caused_by`, `solves`, `leads_to`, `related_to`, `required_by`, `conflicts_with` (unknown → rejected)
- `code_reference`: `"path"`, `"path:line"`, `"path:start-end"`, or `{"file": "...", "line": 42, "function": "..."}`; anything else is rejected
- At least one entity and one relation; both relation ends must appear in `entities`; no self-relations.

Response (MCP): a success message — single mode "Context updated successfully", bulk mode a success/error count. No entry ID is returned; look it up later via `semantic_search` (`content_id`). An error message explains what was rejected (missing entities/relations, unknown relation type, bad `code_reference`).

## assemble_context

| Field | Notes |
|---|---|
| `query` (required) | name the components you care about |
| `session_id` or `workspace_id` | one of them |
| `token_budget` | default 4000 |
| `select` / `select_map` | e.g. `$.formatted_text` to get only the prompt-ready block |

No embeddings: matches entity names in the query, expands one hop, ranks recent updates by entity matches and recency. Returns `items`, `entity_context` (`DirectMention` / `GraphNeighbor{via, relation}`), `impact` (reverse dependencies), `total_tokens`, `formatted_text`.

## query_conversation_context

`session_id`, `query_type`, `parameters` (object; **values are strings**, e.g. `{"limit": "10"}`).

| query_type | parameters |
|---|---|
| `decisions` / `key_decisions` | — |
| `key_insights` | `limit` |
| `open_questions` | — |
| `structured_summary` | — |
| `session_statistics` | — |
| `recent_changes` | `since` (RFC 3339) |
| `code_references` | `file_path` |
| `search_updates` | `query` (keyword match over the full update log) |
| `find_related_content` | `topic` (or `query`), `max_results` |
| `all_entities` | — |
| `find_entities_by_type` | `entity_type` |
| `find_related_entities` / `related_entities` | `entity_name` |
| `get_entity_context` / `entity_context` | `entity_name` |
| `trace_relationships` | `entity_name`, `max_depth` (default 3) |
| `get_entity_network` | `entity_name`, `max_depth` (default 2) |
| `entity_network` | `center_entity`, `max_entities`, `max_relationships` (bounded, keeps direction) |
| `entity_importance` | `limit`, `min_importance` |
| `get_most_important_entities` | `limit` (default 10) |
| `get_recently_mentioned_entities` | `limit` (default 10) |
| `analyze_entity_importance` | — |
| `assemble_context` | `query`, `token_budget` |

## get_structured_summary

`session_id` (required); `include`: array of `decisions`, `insights`, `entities`, `questions`, `all` (default `all`); `decisions_limit`, `entities_limit`, `questions_limit`, `concepts_limit`; `min_confidence` (0–1); `compact` (bool — shorter output, good for catching up).

## manage_entity

`session_id`, `action`:
- `delete` + `entity_name` — removes the entity and cascades its typed edges.
- `delete_update` + `entry_id` — removes one context entry (and its embedding).

## admin

`action`: `health` | `vectorize_session` (+ `session_id`) | `vectorize_stats` | `create_checkpoint` (+ `session_id`).

## Response shape

Tool results are text: a one-line summary followed by `Results:` and compact JSON. Use `select` / `select_map` where offered to cut the JSON down.
