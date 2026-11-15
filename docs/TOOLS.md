# Tools reference

Generated from the operation table (`post-cortex-memory`, `tools::operations::OPERATIONS`) by a test of `post-cortex-memory`; do not edit by hand. A test fails when this file and the table differ.

## Operations

| Operation | Capability method | MCP | gRPC | REST | Response |
|---|---|---|---|---|---|
| `session.create` | `sessions::create` | `session` action=create | CreateSession | `POST /api/sessions` | `text` |
| `session.list` | `sessions::list_summaries` | `session` action=list | ListSessions | `GET /api/sessions` | `SessionList` |
| `session.get` | `sessions::get` | `session` action=load | LoadSession | - | `SessionInfo` |
| `session.find` | `sessions::find` | `session` action=search | SearchSessions | - | `SessionSearch` |
| `session.update` | `sessions::update_metadata` | `session` action=update_metadata | UpdateSessionMetadata | - | `text` |
| `session.delete` | `sessions::delete` | `session` action=delete | DeleteSession | `DELETE /api/sessions/{session_id}` | `text` |
| `hook.event` | `MemoryModule::on_event` | - | - | `POST /api/events` | `Outcome` |
| `message.append` | `messages::append` | `manage_messages` action=append | AppendMessages | `POST /api/messages` | `AppendMessagesResponse` |
| `message.page` | `messages::page` | `manage_messages` action=page | ListMessages | - | `MessagePage` |
| `message.delete` | `messages::delete` | `session` action=delete_message | DeleteMessage | - | `MessagesDeleted` |
| `message.delete_all` | `messages::delete_all` | `session` action=delete_messages | DeleteMessages | - | `MessagesDeleted` |
| `project.resolve` | `projects::resolve` | `session` action=resolve_project | ResolveProject | `POST /api/projects/resolve` | `ResolveProjectResponse` |
| `project.find` | `projects::find` | - | FindProject | - | `FindProjectResponse` |
| `record.write` | `records::write` | `update_conversation_context` | UpdateContext | - | `RecordWritten` |
| `record.bulk_write` | `records::bulk_write` | `update_conversation_context` with `updates` | BulkUpdateContext | - | `BulkWritten` |
| `record.validate` | `records::validate` | `update_conversation_context` with `dry_run` | - | - | `text` |
| `record.list` | `records::list` | - | QueryContext | - | `RecordPage` |
| `entity.delete` | `entities::delete` | `manage_entity` action=delete | DeleteEntity | - | `EntityDeleted` |
| `entity.merge` | `entities::merge` | `manage_entity` action=merge | MergeEntities | - | `EntitiesMerged` |
| `record.delete` | `records::delete` | `manage_entity` action=delete_update | - | - | `RecordChanged` |
| `record.forget` | `records::forget` | `manage_entity` action=forget | ForgetUpdate | - | `RecordChanged` |
| `record.purge` | `records::purge` | `manage_entity` action=purge | PurgeUpdate | - | `RecordChanged` |
| `search` | `search::search` | `semantic_search` | SemanticSearch | `POST /api/search` | `SearchResponse` |
| `search.related` | `search::related` | `query_conversation_context` query_type=find_related_content | FindRelatedContent | - | `RelatedContent` |
| `assemble` | `assembly::assemble` | `assemble_context`; `query_conversation_context` query_type=assemble_context | AssembleContext | - | `Assembled` |
| `view` | `views::view` | `query_conversation_context` query_type=entity\|impact\|lineage\|topics\|radar\|file | GetGraphView | `GET /api/file` | `RenderedView` |
| `summary` | `views::summary` | `get_structured_summary` | GetStructuredSummary | - | `SummaryReply` |
| `summary.decisions` | `views::key_decisions` | `query_conversation_context` query_type=key_decisions | GetKeyDecisions | - | `Vec<DecisionSummary>` |
| `summary.insights` | `views::key_insights` | `query_conversation_context` query_type=key_insights | GetKeyInsights | - | `Vec<String>` |
| `entity.importance` | `views::entity_importance` | `query_conversation_context` query_type=entity_importance | GetEntityImportance | - | `EntityImportance` |
| `entity.network` | `views::entity_network` | `query_conversation_context` query_type=entity_network | GetEntityNetwork | - | `EntityNetworkView` |
| `session.statistics` | `views::statistics` | `query_conversation_context` query_type=session_statistics | GetSessionStatistics | - | `SessionStatistics` |
| `query` | `views::query` | `query_conversation_context` query_type=recent_changes\|code_references\|structured_summary\|decisions\|open_questions\|related_entities\|entity_context\|all_entities\|trace_relationships\|find_related_entities\|get_entity_context\|get_entity_network\|get_most_important_entities\|get_recently_mentioned_entities\|analyze_entity_importance\|find_entities_by_type\|search_updates | - | - | `ContextResponse` |
| `workspace.create` | `workspaces::create` | `manage_workspace` action=create | CreateWorkspace | `POST /api/workspaces` | `WorkspaceCreated` |
| `workspace.list` | `workspaces::list` | `manage_workspace` action=list | ListWorkspaces | `GET /api/workspaces` | `WorkspaceList` |
| `workspace.get` | `workspaces::get` | `manage_workspace` action=get | GetWorkspace | - | `WorkspaceDetail` |
| `workspace.delete` | `workspaces::delete` | `manage_workspace` action=delete | DeleteWorkspace | `DELETE /api/workspaces/{workspace_id}` | `WorkspaceDeleted` |
| `workspace.add_session` | `workspaces::add_session` | `manage_workspace` action=add_session | AddSessionToWorkspace | `POST /api/workspaces/{workspace_id}/sessions/{session_id}` | `MemberAdded` |
| `workspace.remove_session` | `workspaces::remove_session` | `manage_workspace` action=remove_session | RemoveSessionFromWorkspace | - | `MemberRemoved` |
| `card.get` | `cards::get` | `manage_workspace` action=get_card | GetCard | `GET /api/cards` | `GetCardResponse` |
| `card.update` | `cards::update` | `manage_workspace` action=update_card | UpdateCard | `PUT /api/cards/user`, `PUT /api/cards/workspace/{workspace_id}` | `UpdateCardResponse` |
| `workspace.update` | `workspaces::update` | `manage_workspace` action=update | UpdateWorkspace | - | `WorkspaceSummary` |
| `workspace.archive` | `workspaces::set_archived` | `manage_workspace` action=archive\|unarchive | ArchiveWorkspace, UnarchiveWorkspace | - | `WorkspaceSummary` |
| `radar.dismiss` | `views::dismiss_radar_item` | `manage_workspace` action=dismiss_radar_item | DismissRadarItem | - | `RadarDismissal` |
| `radar.restore` | `views::restore_radar_item` | `manage_workspace` action=restore_radar_item | RestoreRadarItem | - | `RadarRestored` |
| `radar.list` | `views::radar_dismissals` | `manage_workspace` action=list_radar_dismissals | ListRadarDismissals | - | `RadarDismissals` |
| `document.put` | `documents::put` | `manage_document` action=put | PutDocument | - | `DocumentWritten` |
| `document.get` | `documents::get` | `manage_document` action=get | GetDocument | - | `DocumentVersion` |
| `document.history` | `documents::history` | `manage_document` action=history | GetDocumentHistory | - | `DocumentHistory` |
| `document.list` | `documents::list` | `manage_document` action=list | ListDocuments | - | `DocumentList` |
| `document.delete` | `documents::delete` | `manage_document` action=delete | DeleteDocument | - | `DocumentDeleted` |
| `blob.put` | `blobs::put` | - | Blobs/PutBlob | - | `BlobInfo` |
| `blob.get` | `blobs::get` | - | Blobs/GetBlob | - | `BlobChunk` |
| `blob.stat` | `blobs::stat` | - | Blobs/StatBlob | - | `BlobInfo` |
| `source.register` | `freshness::register` | - | RegisterSource | - | `RegisterSourceAck` |
| `source.register_batch` | `freshness::register_batch` | - | RegisterSourceBatch | - | `RegisterSourceBatchAck` |
| `freshness.check` | `freshness::check_batch` | - | CheckFreshness | - | `FreshnessReport` |
| `freshness.invalidate` | `freshness::invalidate` | - | Invalidate | - | `InvalidateAck` |
| `freshness.symbol_dependency` | `freshness::register_symbol_dependencies` | - | RegisterSymbolDependency | - | `RegisterSymbolDependencyAck` |
| `freshness.cascade` | `freshness::cascade_invalidate` | - | CascadeInvalidate | - | `CascadeInvalidateReport` |
| `freshness.stale_entries` | `freshness::stale_entries` | - | GetStaleEntriesBySource | - | `GetStaleEntriesBySourceResponse` |
| `export` | `transfer::plan_export` | - | Export | - | `ExportChunk` |
| `import` | `transfer::import` | - | Import | - | `ImportResponse` |
| `health` | `admin::health` | `admin` action=health | Health | `GET /health` | `HealthPayload` |
| `vectorize.session` | `admin::vectorize_session` | `admin` action=vectorize_session | VectorizeSession | - | `Vectorized` |
| `vectorize.stats` | `admin::vectorization_stats` | `admin` action=vectorize_stats | GetVectorizationStats | - | `VectorizationStats` |
| `session.checkpoint` | `sessions::checkpoint` | `admin` action=create_checkpoint | CreateCheckpoint | - | `Checkpoint` |
| `views.graph` | `views::DetailViews::graph` | - | Views/GetGraph | - | `Graph` |
| `views.timeline` | `views::DetailViews::timeline` | - | Views/GetTimeline | - | `Timeline` |
| `views.records` | `views::DetailViews::records` | - | Views/ListRecords | - | `RecordPage` |
| `views.record` | `views::DetailViews::record` | `query_conversation_context` query_type=record | Views/GetRecord | - | `Record` |
| `views.records_of` | `views::DetailViews::records_of` | - | Views/GetRecords | - | `Records` |
| `views.overview` | `views::DetailViews::overview` | - | Views/GetOverview | - | `Overview` |
| `views.entity` | `views::DetailViews::entity` | - | Views/GetEntity | - | `EntityDetail` |
| `views.search` | `views::DetailViews::search` | - | Views/Search | - | `SearchResponse` |
| `views.topics` | `views::DetailViews::topics` | - | Views/ListTopics | - | `Topics` |
| `views.radar` | `views::DetailViews::radar` | - | Views/GetRadar | - | `Radar` |
| `views.file` | `views::DetailViews::file` | - | Views/GetFileView | - | `FileView` |
| `views.files` | `views::DetailViews::files` | - | Views/ListFiles | - | `Files` |
| `views.conversations` | `views::DetailViews::conversations` | - | Views/ListConversations | - | `Conversations` |
| `views.conversation` | `views::DetailViews::conversation` | - | Views/GetConversation | - | `Conversation` |
| `views.watch` | `views::DetailViews::watch` | - | Views/WatchActivity | - | `ActivityEvent` |

## Effects

What each operation does to memory, and the request fields that name its sessions and workspaces (none: the whole instance). A gateway admits, limits and counts calls from these.

| Operation | Effect | Sessions | Workspaces |
|---|---|---|---|
| `session.create` | write | - | - |
| `session.list` | read | - | - |
| `session.get` | read | `session_id` | - |
| `session.find` | read | - | - |
| `session.update` | write | `session_id` | - |
| `session.delete` | delete | `session_id` | - |
| `hook.event` | write | `scope.session_id` | `scope.workspace_id` |
| `message.append` | write (+messages per `messages`) | `session_id` | - |
| `message.page` | read | `session_id` | - |
| `message.delete` | delete (-messages answer `deleted`) | `session_id` | - |
| `message.delete_all` | delete (-messages answer `deleted`) | `session_id` | - |
| `project.resolve` | write | - | `workspace` |
| `project.find` | read | - | `workspace` |
| `record.write` | write (+records 1) | `session_id` | - |
| `record.bulk_write` | write (+records per `updates`) | `session_id` | - |
| `record.validate` | read | `session_id` | - |
| `record.list` | read | `session_id` | - |
| `entity.delete` | delete | `session_id` | - |
| `entity.merge` | write | `session_id` | - |
| `record.delete` | delete (-records 1) | `session_id` | - |
| `record.forget` | delete | `session_id` | - |
| `record.purge` | delete (-records 1) | `session_id` | - |
| `search` | search, reranked when `mode` is local\|accurate | `session_id` | `workspace_id` |
| `search.related` | search | `session_id` | - |
| `assemble` | search | `session_id` | `workspace_id` |
| `view` | read | `session_id` | `workspace_id` |
| `summary` | read | `session_id` | - |
| `summary.decisions` | read | `session_id` | - |
| `summary.insights` | read | `session_id` | - |
| `entity.importance` | read | `session_id` | - |
| `entity.network` | read | `session_id` | - |
| `session.statistics` | read | `session_id` | - |
| `query` | read | `session_id` | - |
| `workspace.create` | write (+workspaces 1) | - | - |
| `workspace.list` | read | - | - |
| `workspace.get` | read | - | `workspace_id` |
| `workspace.delete` | delete (-workspaces 1) | - | `workspace_id` |
| `workspace.add_session` | write | `session_id` | `workspace_id` |
| `workspace.remove_session` | write | `session_id` | `workspace_id` |
| `card.get` | read | - | `workspace_id` |
| `card.update` | write | - | `workspace_id` |
| `workspace.update` | write | - | `workspace_id` |
| `workspace.archive` | write | - | `workspace_id` |
| `radar.dismiss` | write | - | `workspace_id` |
| `radar.restore` | delete | - | `workspace_id` |
| `radar.list` | read | - | `workspace_id` |
| `document.put` | write | - | `workspace_id` |
| `document.get` | read | - | `workspace_id` |
| `document.history` | read | - | `workspace_id` |
| `document.list` | read | - | `workspace_id` |
| `document.delete` | delete | - | `workspace_id` |
| `blob.put` | write | - | `workspace_id` |
| `blob.get` | read | - | `workspace_id` |
| `blob.stat` | read | - | `workspace_id` |
| `source.register` | write | `session_id` | - |
| `source.register_batch` | write | `session_id` | - |
| `freshness.check` | read | - | - |
| `freshness.invalidate` | write | `session_id` | - |
| `freshness.symbol_dependency` | write | - | - |
| `freshness.cascade` | write | - | - |
| `freshness.stale_entries` | read | - | - |
| `export` | read | `session_ids` | `workspace_id` |
| `import` | write when `settings.commit`, else read (+records answer `updates_imported`) (+workspaces answer `workspaces_imported`) (+messages answer `messages_imported`) | - | - |
| `health` | read | - | - |
| `vectorize.session` | write | `session_id` | - |
| `vectorize.stats` | read | - | - |
| `session.checkpoint` | write | `session_id` | - |
| `views.graph` | read | `scope.session_id` | `scope.workspace_id` |
| `views.timeline` | read | `scope.session_id` | `scope.workspace_id` |
| `views.records` | read | `scope.session_id`, `session_id` | `scope.workspace_id` |
| `views.record` | read | `scope.session_id` | `scope.workspace_id` |
| `views.records_of` | read | `scope.session_id` | `scope.workspace_id` |
| `views.overview` | read | `scope.session_id` | `scope.workspace_id` |
| `views.entity` | read | `scope.session_id` | `scope.workspace_id` |
| `views.search` | search, reranked when `mode` is SEARCH_MODE_LOCAL\|SEARCH_MODE_ACCURATE | `scope.session_id` | `scope.workspace_id` |
| `views.topics` | read | `scope.session_id` | `scope.workspace_id` |
| `views.radar` | read | `scope.session_id` | `scope.workspace_id` |
| `views.file` | read | `scope.session_id` | `scope.workspace_id` |
| `views.files` | read | `scope.session_id` | `scope.workspace_id` |
| `views.conversations` | read | `scope.session_id` | `scope.workspace_id` |
| `views.conversation` | read | `session_id` | - |
| `views.watch` | read | `scope.session_id` | `scope.workspace_id` |

## MCP tools

### `admin`

Daemon administration. Actions: health, vectorize_session (session_id), vectorize_stats, create_checkpoint (session_id).

Operations: `health` (Health of the memory instance); `vectorize.session` (Embed a session's records); `vectorize.stats` (Embedding statistics); `session.checkpoint` (Snapshot a session).

Arguments: `action` (required), `session_id`.

### `assemble_context`

Everything for one task in one call, within token_budget: the user and project cards first, then the graph entities named in the query with their one-hop neighbourhood and what depends on them (at most half of what the cards leave), then the records that best match the query, ranked by the same search as semantic_search (only records that still hold, each shown as its passage for the query). With sources=messages or all, also adds captured conversation turns matching the query as excerpts. Returns ranked items, entity neighbourhood, dependency impact, and an LLM-ready formatted block. Scope: session or workspace.

Operations: `assemble` (Graph-aware context for a query).

Arguments: `query` (required), `select`, `select_map`, `session_id`, `sources`, `token_budget`, `workspace_id`.

### `get_structured_summary`

Get session summary. Use 'include' to specify sections: decisions, insights, entities, or all (default).

Operations: `summary` (The structured summary of a session).

Arguments: `compact`, `concepts_limit`, `decisions_limit`, `entities_limit`, `include`, `min_confidence`, `questions_limit`, `session_id` (required).

### `manage_document`

The documents modules keep in a workspace, versioned. Actions: put (kind, data, text, expected_version; document_id for a change), get, history, list (kind, fields, limit, cursor), delete. Only the module that declares a kind writes it; anyone who reads the workspace reads it.

Operations: `document.put` (Write a module's document (versioned)); `document.get` (A document, latest or one version); `document.history` (The versions of a document); `document.list` (The documents of a kind, filtered and paged); `document.delete` (Delete a document with every version).

Arguments: `action` (required), `author`, `cursor`, `data`, `document_id`, `expected_version`, `fields`, `kind`, `limit`, `text`, `version`, `workspace_id` (required).

### `manage_messages`

A session's message stream, its conversation turns. Actions: append (messages: each with turn_id, role, text, and for an agent's turn its parent and blocks), page (from, limit; oldest first, next_from for the next page). Agents keep their turns whole here to resume them.

Operations: `message.append` (Append conversation turns); `message.page` (A page of a session's turns, oldest first).

Arguments: `action` (required), `from`, `limit`, `messages`, `session_id` (required).

### `manage_entity`

Manage entities and context entries in a session. Actions: delete (remove entity + cascade typed edges; requires entity_name), delete_update (remove a single context entry by entry_id).

Operations: `entity.delete` (Delete an entity and its edges); `entity.merge` (Merge an entity into another (alias)); `record.delete` (Delete a record); `record.forget` (Retire a record that no longer holds); `record.purge` (Erase a record and what derives from it).

Arguments: `action` (required), `entity_name`, `entry_id`, `into`, `session_id` (required).

### `manage_workspace`

Manage workspaces and their cards. Actions: create, list, get, delete, add_session, remove_session; get_card / update_card for the project card (workspace_id) or the user card (card: "user"): short markdown notes put in front of every session

Operations: `workspace.create` (Create a workspace); `workspace.list` (The workspaces); `workspace.get` (A workspace with its members); `workspace.delete` (Delete a workspace (its sessions stay)); `workspace.add_session` (Add a session to a workspace); `workspace.remove_session` (Remove a session from a workspace); `card.get` (The user card or a project card, with history); `card.update` (Write a card (versioned)); `workspace.update` (Rename or redescribe a workspace); `workspace.archive` (Archive or unarchive a workspace); `radar.dismiss` (Dismiss a radar item of a workspace); `radar.restore` (Restore a dismissed radar item); `radar.list` (The radar dismissals of a workspace).

Arguments: `action` (required), `author`, `card`, `content`, `description`, `expected_version`, `history`, `include_archived`, `key`, `kind`, `name`, `role`, `session_id`, `workspace_id`.

### `query_conversation_context`

Query session data. Types include: find_related_entities, get_entity_context, search_updates, entity_importance, entity_network, find_related_content, key_decisions, key_insights, session_statistics, structured_summary, decisions, open_questions, assemble_context, record (one record whole by entry_id, e.g. after a search snippet).

Operations: `search.related` (Content related to a topic in a session); `assemble` (Graph-aware context for a query); `view` (A graph view: entity, impact, lineage, topics, radar, file); `summary.decisions` (A session's decisions, oldest first); `summary.insights` (A session's key insights); `entity.importance` (Entities ranked by importance); `entity.network` (The entity network around a center); `session.statistics` (Size and activity of a session); `query` (Typed queries over a session's records and graph); `views.record` (One record with its context).

Arguments: `parameters` (required), `query_type` (required), `session_id` (required).

### `semantic_search`

Universal semantic search. Scope: session (requires scope_id), workspace (requires scope_id), or global (default).

Operations: `search` (Semantic search in a session, workspace or everything).

Arguments: `author`, `date_from`, `date_to`, `detail`, `interaction_type`, `limit`, `mode`, `prefer_author`, `query` (required), `recency_bias`, `scope`, `scope_id`, `select`, `select_map`, `sources`, `trace`.

### `session`

Manage sessions. Actions: create, list, load, search, update_metadata, delete; resolve_project (key, name, optional workspace) finds or creates the workspace and session of a project.

Operations: `session.create` (Create a session); `session.list` (List the sessions (metadata only)); `session.get` (Load a session's metadata); `session.find` (Find sessions by name or description); `session.update` (Rename or redescribe a session); `session.delete` (Delete a session with its records); `message.delete` (Delete one captured turn); `message.delete_all` (Delete a session's captured turns); `project.resolve` (Find or create the workspace and session of a project key).

Arguments: `action` (required), `description`, `key`, `name`, `query`, `seq`, `session_id`, `workspace`.

### `update_conversation_context`

Add context updates to conversation. Supports single update or bulk mode with updates array.

Operations: `record.write` (Write one record); `record.bulk_write` (Write records atomically, ids in input order); `record.validate` (Run every check of a write, store nothing (dry run)).

Arguments: `author`, `code_reference`, `confidence`, `content`, `derived_from`, `dry_run`, `entities`, `interaction_type`, `relations`, `session_id` (required), `supersedes`, `updates`, `valid_from`.
