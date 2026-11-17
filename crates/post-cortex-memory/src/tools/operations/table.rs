// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The rows of the operation table.

use crate::tools::operations::{
    Count, Counter, Effect, McpRoute, Operation, RERANKED, Rerank, ScopeFields, Tally,
};

/// An MCP route.
const fn mcp(tool: &'static str, values: &'static [&'static str]) -> McpRoute {
    McpRoute { tool, values }
}

/// A row: what the operation is and where transports expose it; its
/// effect and scope follow ([`Row::does`]).
struct Row {
    name: &'static str,
    capability: &'static str,
    summary: &'static str,
    response: &'static str,
    mcp: &'static [McpRoute],
    grpc: &'static [&'static str],
    views: &'static [&'static str],
    blobs: &'static [&'static str],
    rest: &'static [&'static str],
}

const fn op(
    name: &'static str,
    capability: &'static str,
    summary: &'static str,
    response: &'static str,
    mcp: &'static [McpRoute],
    grpc: &'static [&'static str],
    rest: &'static [&'static str],
) -> Row {
    Row {
        name,
        capability,
        summary,
        response,
        mcp,
        grpc,
        views: &[],
        blobs: &[],
        rest,
    }
}

/// A row of the `pcx.views.v1` service: one RPC, no other transport.
const fn view(
    name: &'static str,
    capability: &'static str,
    summary: &'static str,
    response: &'static str,
    rpc: &'static [&'static str],
) -> Row {
    Row {
        name,
        capability,
        summary,
        response,
        mcp: &[],
        grpc: &[],
        views: rpc,
        blobs: &[],
        rest: &[],
    }
}

/// A row of the `pcx.v1.Blobs` service: one RPC, no other transport.
const fn blob(
    name: &'static str,
    capability: &'static str,
    summary: &'static str,
    response: &'static str,
    rpc: &'static [&'static str],
) -> Row {
    Row {
        name,
        capability,
        summary,
        response,
        mcp: &[],
        grpc: &[],
        views: &[],
        blobs: rpc,
        rest: &[],
    }
}

impl Row {
    /// The operation, with what it does to memory and its scope fields.
    const fn does(self, effect: Effect, scope: ScopeFields) -> Operation {
        Operation {
            name: self.name,
            capability: self.capability,
            summary: self.summary,
            response: self.response,
            mcp: self.mcp,
            grpc: self.grpc,
            views: self.views,
            blobs: self.blobs,
            rest: self.rest,
            effect,
            scope,
        }
    }
}

/// Reads only.
const READ: Effect = Effect::Read;

/// A search that never reranks.
const SEARCH: Effect = Effect::Search { rerank: None };

/// A search whose `mode` decides the reranker.
const SEARCH_RERANKS: Effect = Effect::Search {
    rerank: Some(Rerank {
        field: "mode",
        values: &["local", "accurate"],
        fallback: "fast",
        answered: RERANKED,
        withheld: "rerank_withheld",
    }),
};

/// A view search whose `mode` decides the reranker.
const VIEW_SEARCH: Effect = Effect::Search {
    rerank: Some(Rerank {
        field: "mode",
        values: &["SEARCH_MODE_LOCAL", "SEARCH_MODE_ACCURATE"],
        fallback: "SEARCH_MODE_FAST",
        answered: RERANKED,
        withheld: "rerank_withheld",
    }),
};

/// A committed import adds the records, workspaces and messages its answer
/// counts; a preview writes nothing.
const IMPORT: Effect = Effect::Write {
    counts: &[
        tally(Counter::Records, Count::Answer("updates_imported")),
        tally(Counter::Workspaces, Count::Answer("workspaces_imported")),
        tally(Counter::Messages, Count::Answer("messages_imported")),
    ],
    commit: Some("settings.commit"),
};

const fn tally(counter: Counter, by: Count) -> Tally {
    Tally { counter, by }
}

const fn write(counts: &'static [Tally]) -> Effect {
    Effect::Write {
        counts,
        commit: None,
    }
}

const fn delete(counts: &'static [Tally]) -> Effect {
    Effect::Delete { counts }
}

const fn scope(
    sessions: &'static [&'static str],
    workspaces: &'static [&'static str],
) -> ScopeFields {
    ScopeFields {
        sessions,
        workspaces,
    }
}

/// The whole instance.
const NONE: ScopeFields = scope(&[], &[]);
const SESSION: ScopeFields = scope(&["session_id"], &[]);
const WORKSPACE: ScopeFields = scope(&[], &["workspace_id"]);
const SESSION_WORKSPACE: ScopeFields = scope(&["session_id"], &["workspace_id"]);
/// A project key placed in a workspace (or the whole instance).
const PROJECT: ScopeFields = scope(&[], &["workspace"]);
const EXPORT: ScopeFields = scope(&["session_ids"], &["workspace_id"]);
/// The scope of a hook event's envelope (`pcx.module.v1`).
const ENVELOPE: ScopeFields = scope(&["scope.session_id"], &["scope.workspace_id"]);
/// The `scope` message of a `pcx.views.v1` request.
const VIEW_SCOPE: ScopeFields = scope(&["scope.session_id"], &["scope.workspace_id"]);
/// A record list: the view scope, or one session of it.
const RECORDS_SCOPE: ScopeFields =
    scope(&["scope.session_id", "session_id"], &["scope.workspace_id"]);

/// Every user-facing operation.
#[rustfmt::skip]
pub static OPERATIONS: &[Operation] = &[
    // Sessions
    op("session.create", "sessions::create", "Create a session", "text",
        &[mcp("session", &["create"])], &["CreateSession"], &["POST /api/sessions"])
        .does(write(&[]), NONE),
    op("session.list", "sessions::list_summaries", "List the sessions (metadata only)", "SessionList",
        &[mcp("session", &["list"])], &["ListSessions"], &["GET /api/sessions"])
        .does(READ, NONE),
    op("session.get", "sessions::get", "Load a session's metadata", "SessionInfo",
        &[mcp("session", &["load"])], &["LoadSession"], &[])
        .does(READ, SESSION),
    op("session.find", "sessions::find", "Find sessions by name or description", "SessionSearch",
        &[mcp("session", &["search"])], &["SearchSessions"], &[])
        .does(READ, NONE),
    op("session.update", "sessions::update_metadata", "Rename or redescribe a session", "text",
        &[mcp("session", &["update_metadata"])], &["UpdateSessionMetadata"], &[])
        .does(write(&[]), SESSION),
    op("session.delete", "sessions::delete", "Delete a session with its records", "text",
        &[mcp("session", &["delete"])], &["DeleteSession"], &["DELETE /api/sessions/{session_id}"])
        .does(delete(&[]), SESSION),
    // Hook events (the module host)
    op("hook.event", "MemoryModule::on_event", "A client hook event: session start cards, file memory, turn capture", "Outcome",
        &[], &[], &["POST /api/events"])
        .does(write(&[]), ENVELOPE),
    // Messages
    op("message.append", "messages::append", "Append conversation turns", "AppendMessagesResponse",
        &[mcp("manage_messages", &["append"])], &["AppendMessages"], &["POST /api/messages"])
        .does(write(&[tally(Counter::Messages, Count::Items("messages"))]), SESSION),
    op("message.page", "messages::page", "A page of a session's turns, oldest first", "MessagePage",
        &[mcp("manage_messages", &["page"])], &["ListMessages"], &[])
        .does(READ, SESSION),
    op("message.delete", "messages::delete", "Delete one captured turn", "MessagesDeleted",
        &[mcp("session", &["delete_message"])], &["DeleteMessage"], &[])
        .does(delete(&[tally(Counter::Messages, Count::Answer("deleted"))]), SESSION),
    op("message.delete_all", "messages::delete_all", "Delete a session's captured turns", "MessagesDeleted",
        &[mcp("session", &["delete_messages"])], &["DeleteMessages"], &[])
        .does(delete(&[tally(Counter::Messages, Count::Answer("deleted"))]), SESSION),
    // Projects
    op("project.resolve", "projects::resolve", "Find or create the workspace and session of a project key", "ResolveProjectResponse",
        &[mcp("session", &["resolve_project"])], &["ResolveProject"], &["POST /api/projects/resolve"])
        .does(write(&[]), PROJECT),
    op("project.find", "projects::find", "Where a project key would resolve, creating nothing", "FindProjectResponse",
        &[], &["FindProject"], &[])
        .does(READ, PROJECT),
    // Records
    op("record.write", "records::write", "Write one record", "RecordWritten",
        &[mcp("update_conversation_context", &[])], &["UpdateContext"], &[])
        .does(write(&[tally(Counter::Records, Count::One)]), SESSION),
    op("record.bulk_write", "records::bulk_write", "Write records atomically, ids in input order", "BulkWritten",
        &[mcp("update_conversation_context", &["updates"])], &["BulkUpdateContext"], &[])
        .does(write(&[tally(Counter::Records, Count::Items("updates"))]), SESSION),
    op("record.validate", "records::validate", "Run every check of a write, store nothing (dry run)", "text",
        &[mcp("update_conversation_context", &["dry_run"])], &[], &[])
        .does(READ, SESSION),
    op("record.list", "records::list", "A session's records by type and time", "RecordPage",
        &[], &["QueryContext"], &[])
        .does(READ, SESSION),
    // Entities and records (manage_entity)
    op("entity.delete", "entities::delete", "Delete an entity and its edges", "EntityDeleted",
        &[mcp("manage_entity", &["delete"])], &["DeleteEntity"], &[])
        .does(delete(&[]), SESSION),
    op("entity.merge", "entities::merge", "Merge an entity into another (alias)", "EntitiesMerged",
        &[mcp("manage_entity", &["merge"])], &["MergeEntities"], &[])
        .does(write(&[]), SESSION),
    op("record.delete", "records::delete", "Delete a record", "RecordChanged",
        &[mcp("manage_entity", &["delete_update"])], &[], &[])
        .does(delete(&[tally(Counter::Records, Count::One)]), SESSION),
    op("record.forget", "records::forget", "Retire a record that no longer holds", "RecordChanged",
        &[mcp("manage_entity", &["forget"])], &["ForgetUpdate"], &[])
        .does(delete(&[]), SESSION),
    op("record.purge", "records::purge", "Erase a record and what derives from it", "RecordChanged",
        &[mcp("manage_entity", &["purge"])], &["PurgeUpdate"], &[])
        .does(delete(&[tally(Counter::Records, Count::One)]), SESSION),
    // Search
    op("search", "search::search", "Semantic search in a session, workspace or everything", "SearchResponse",
        &[mcp("semantic_search", &[])], &["SemanticSearch"], &["POST /api/search"])
        .does(SEARCH_RERANKS, SESSION_WORKSPACE),
    op("search.related", "search::related", "Content related to a topic in a session", "RelatedContent",
        &[mcp("query_conversation_context", &["find_related_content"])], &["FindRelatedContent"], &[])
        .does(SEARCH, SESSION),
    // Assembly
    op("assemble", "assembly::assemble", "Graph-aware context for a query", "Assembled",
        &[mcp("assemble_context", &[]), mcp("query_conversation_context", &["assemble_context"])],
        &["AssembleContext"], &[])
        .does(SEARCH, SESSION_WORKSPACE),
    // Views
    op("view", "views::view", "A graph view: entity, impact, lineage, topics, radar, file", "RenderedView",
        &[mcp("query_conversation_context", &["entity", "impact", "lineage", "topics", "radar", "file"])],
        &["GetGraphView"], &["GET /api/file"])
        .does(READ, SESSION_WORKSPACE),
    op("summary", "views::summary", "The structured summary of a session", "SummaryReply",
        &[mcp("get_structured_summary", &[])], &["GetStructuredSummary"], &[])
        .does(READ, SESSION),
    op("summary.decisions", "views::key_decisions", "A session's decisions, oldest first", "Vec<DecisionSummary>",
        &[mcp("query_conversation_context", &["key_decisions"])], &["GetKeyDecisions"], &[])
        .does(READ, SESSION),
    op("summary.insights", "views::key_insights", "A session's key insights", "Vec<String>",
        &[mcp("query_conversation_context", &["key_insights"])], &["GetKeyInsights"], &[])
        .does(READ, SESSION),
    op("entity.importance", "views::entity_importance", "Entities ranked by importance", "EntityImportance",
        &[mcp("query_conversation_context", &["entity_importance"])], &["GetEntityImportance"], &[])
        .does(READ, SESSION),
    op("entity.network", "views::entity_network", "The entity network around a center", "EntityNetworkView",
        &[mcp("query_conversation_context", &["entity_network"])], &["GetEntityNetwork"], &[])
        .does(READ, SESSION),
    op("session.statistics", "views::statistics", "Size and activity of a session", "SessionStatistics",
        &[mcp("query_conversation_context", &["session_statistics"])], &["GetSessionStatistics"], &[])
        .does(READ, SESSION),
    op("query", "views::query", "Typed queries over a session's records and graph", "ContextResponse",
        &[mcp("query_conversation_context", &[
            "recent_changes", "code_references", "structured_summary", "decisions",
            "open_questions", "related_entities", "entity_context", "all_entities",
            "trace_relationships", "find_related_entities", "get_entity_context",
            "get_entity_network", "get_most_important_entities",
            "get_recently_mentioned_entities", "analyze_entity_importance",
            "find_entities_by_type", "search_updates",
        ])], &[], &[])
        .does(READ, SESSION),
    // Workspaces, cards and radar dismissals (manage_workspace)
    op("workspace.create", "workspaces::create", "Create a workspace", "WorkspaceCreated",
        &[mcp("manage_workspace", &["create"])], &["CreateWorkspace"], &["POST /api/workspaces"])
        .does(write(&[tally(Counter::Workspaces, Count::One)]), NONE),
    op("workspace.list", "workspaces::list", "The workspaces", "WorkspaceList",
        &[mcp("manage_workspace", &["list"])], &["ListWorkspaces"], &["GET /api/workspaces"])
        .does(READ, NONE),
    op("workspace.get", "workspaces::get", "A workspace with its members", "WorkspaceDetail",
        &[mcp("manage_workspace", &["get"])], &["GetWorkspace"], &[])
        .does(READ, WORKSPACE),
    op("workspace.delete", "workspaces::delete", "Delete a workspace (its sessions stay)", "WorkspaceDeleted",
        &[mcp("manage_workspace", &["delete"])], &["DeleteWorkspace"], &["DELETE /api/workspaces/{workspace_id}"])
        .does(delete(&[tally(Counter::Workspaces, Count::One)]), WORKSPACE),
    op("workspace.add_session", "workspaces::add_session", "Add a session to a workspace", "MemberAdded",
        &[mcp("manage_workspace", &["add_session"])], &["AddSessionToWorkspace"],
        &["POST /api/workspaces/{workspace_id}/sessions/{session_id}"])
        .does(write(&[]), SESSION_WORKSPACE),
    op("workspace.remove_session", "workspaces::remove_session", "Remove a session from a workspace", "MemberRemoved",
        &[mcp("manage_workspace", &["remove_session"])], &["RemoveSessionFromWorkspace"], &[])
        .does(write(&[]), SESSION_WORKSPACE),
    op("card.get", "cards::get", "The user card or a project card, with history", "GetCardResponse",
        &[mcp("manage_workspace", &["get_card"])], &["GetCard"], &["GET /api/cards"])
        .does(READ, WORKSPACE),
    op("card.update", "cards::update", "Write a card (versioned)", "UpdateCardResponse",
        &[mcp("manage_workspace", &["update_card"])], &["UpdateCard"],
        &["PUT /api/cards/user", "PUT /api/cards/workspace/{workspace_id}"])
        .does(write(&[]), WORKSPACE),
    op("workspace.update", "workspaces::update", "Rename or redescribe a workspace", "WorkspaceSummary",
        &[mcp("manage_workspace", &["update"])], &["UpdateWorkspace"], &[])
        .does(write(&[]), WORKSPACE),
    op("workspace.archive", "workspaces::set_archived", "Archive or unarchive a workspace", "WorkspaceSummary",
        &[mcp("manage_workspace", &["archive", "unarchive"])], &["ArchiveWorkspace", "UnarchiveWorkspace"], &[])
        .does(write(&[]), WORKSPACE),
    op("radar.dismiss", "views::dismiss_radar_item", "Dismiss a radar item of a workspace", "RadarDismissal",
        &[mcp("manage_workspace", &["dismiss_radar_item"])], &["DismissRadarItem"], &[])
        .does(write(&[]), WORKSPACE),
    op("radar.restore", "views::restore_radar_item", "Restore a dismissed radar item", "RadarRestored",
        &[mcp("manage_workspace", &["restore_radar_item"])], &["RestoreRadarItem"], &[])
        .does(delete(&[]), WORKSPACE),
    op("radar.list", "views::radar_dismissals", "The radar dismissals of a workspace", "RadarDismissals",
        &[mcp("manage_workspace", &["list_radar_dismissals"])], &["ListRadarDismissals"], &[])
        .does(READ, WORKSPACE),
    // Documents of modules (manage_document)
    op("document.put", "documents::put", "Write a module's document (versioned)", "DocumentWritten",
        &[mcp("manage_document", &["put"])], &["PutDocument"], &[])
        .does(write(&[]), WORKSPACE),
    op("document.get", "documents::get", "A document, latest or one version", "DocumentVersion",
        &[mcp("manage_document", &["get"])], &["GetDocument"], &[])
        .does(READ, WORKSPACE),
    op("document.history", "documents::history", "The versions of a document", "DocumentHistory",
        &[mcp("manage_document", &["history"])], &["GetDocumentHistory"], &[])
        .does(READ, WORKSPACE),
    op("document.list", "documents::list", "The documents of a kind, filtered and paged", "DocumentList",
        &[mcp("manage_document", &["list"])], &["ListDocuments"], &[])
        .does(READ, WORKSPACE),
    op("document.delete", "documents::delete", "Delete a document with every version", "DocumentDeleted",
        &[mcp("manage_document", &["delete"])], &["DeleteDocument"], &[])
        .does(delete(&[]), WORKSPACE),
    // Blobs: the images documents name (W24)
    blob("blob.put", "blobs::put", "Keep an image of a workspace (stream)", "BlobInfo",
        &["PutBlob"])
        .does(write(&[]), WORKSPACE),
    blob("blob.get", "blobs::get", "The bytes of an image (stream)", "BlobChunk",
        &["GetBlob"])
        .does(READ, WORKSPACE),
    blob("blob.stat", "blobs::stat", "What an image is", "BlobInfo",
        &["StatBlob"])
        .does(READ, WORKSPACE),
    // Freshness
    op("source.register", "freshness::register", "Register a record's source reference", "RegisterSourceAck",
        &[], &["RegisterSource"], &[])
        .does(write(&[]), SESSION),
    op("source.register_batch", "freshness::register_batch", "Register source references", "RegisterSourceBatchAck",
        &[], &["RegisterSourceBatch"], &[])
        .does(write(&[]), SESSION),
    op("freshness.check", "freshness::check_batch", "Which source references are stale", "FreshnessReport",
        &[], &["CheckFreshness"], &[])
        .does(READ, NONE),
    op("freshness.invalidate", "freshness::invalidate", "Invalidate a file's records in a session", "InvalidateAck",
        &[], &["Invalidate"], &[])
        .does(write(&[]), SESSION),
    op("freshness.symbol_dependency", "freshness::register_symbol_dependencies", "Register a symbol dependency", "RegisterSymbolDependencyAck",
        &[], &["RegisterSymbolDependency"], &[])
        .does(write(&[]), NONE),
    op("freshness.cascade", "freshness::cascade_invalidate", "Invalidate a symbol and its dependents", "CascadeInvalidateReport",
        &[], &["CascadeInvalidate"], &[])
        .does(write(&[]), NONE),
    op("freshness.stale_entries", "freshness::stale_entries", "Stale entries of a source", "GetStaleEntriesBySourceResponse",
        &[], &["GetStaleEntriesBySource"], &[])
        .does(READ, NONE),
    // Transfer
    op("export", "transfer::plan_export", "Export sessions and workspaces (stream)", "ExportChunk",
        &[], &["Export"], &[])
        .does(READ, EXPORT),
    op("import", "transfer::import", "Import an export (stream)", "ImportResponse",
        &[], &["Import"], &[])
        .does(IMPORT, NONE),
    // Admin
    op("health", "admin::health", "Health of the memory instance", "HealthPayload",
        &[mcp("admin", &["health"])], &["Health"], &["GET /health"])
        .does(READ, NONE),
    op("vectorize.session", "admin::vectorize_session", "Embed a session's records", "Vectorized",
        &[mcp("admin", &["vectorize_session"])], &["VectorizeSession"], &[])
        .does(write(&[]), SESSION),
    op("vectorize.stats", "admin::vectorization_stats", "Embedding statistics", "VectorizationStats",
        &[mcp("admin", &["vectorize_stats"])], &["GetVectorizationStats"], &[])
        .does(READ, NONE),
    op("session.checkpoint", "sessions::checkpoint", "Snapshot a session", "Checkpoint",
        &[mcp("admin", &["create_checkpoint"])], &["CreateCheckpoint"], &[])
        .does(write(&[]), SESSION),
    // The views of the app (pcx.views.v1)
    view("views.graph", "views::DetailViews::graph", "The entity graph of a scope", "Graph", &["GetGraph"])
        .does(READ, VIEW_SCOPE),
    view("views.timeline", "views::DetailViews::timeline", "A scope's records over time", "Timeline", &["GetTimeline"])
        .does(READ, VIEW_SCOPE),
    view("views.records", "views::DetailViews::records", "A scope's records, filtered and paged", "RecordPage", &["ListRecords"])
        .does(READ, RECORDS_SCOPE),
    Row {
        mcp: &[mcp("query_conversation_context", &["record"])],
        ..view("views.record", "views::DetailViews::record", "One record with its context", "Record", &["GetRecord"])
    }
    .does(READ, VIEW_SCOPE),
    view("views.records_of", "views::DetailViews::records_of", "Several records by their ids, in one read", "Records", &["GetRecords"])
        .does(READ, VIEW_SCOPE),
    view("views.overview", "views::DetailViews::overview", "A scope at a glance", "Overview", &["GetOverview"])
        .does(READ, VIEW_SCOPE),
    view("views.entity", "views::DetailViews::entity", "An entity with its impact and lineage", "EntityDetail", &["GetEntity"])
        .does(READ, VIEW_SCOPE),
    view("views.search", "views::DetailViews::search", "Search records and messages of a scope", "SearchResponse", &["Search"])
        .does(VIEW_SEARCH, VIEW_SCOPE),
    view("views.topics", "views::DetailViews::topics", "The topics of a scope", "Topics", &["ListTopics"])
        .does(READ, VIEW_SCOPE),
    view("views.radar", "views::DetailViews::radar", "What needs attention in a scope", "Radar", &["GetRadar"])
        .does(READ, VIEW_SCOPE),
    view("views.file", "views::DetailViews::file", "What a scope knows about a file", "FileView", &["GetFileView"])
        .does(READ, VIEW_SCOPE),
    view("views.files", "views::DetailViews::files", "The files a scope mentions", "Files", &["ListFiles"])
        .does(READ, VIEW_SCOPE),
    view("views.conversations", "views::DetailViews::conversations", "The captured conversations of a scope", "Conversations", &["ListConversations"])
        .does(READ, VIEW_SCOPE),
    view("views.conversation", "views::DetailViews::conversation", "One conversation's turns, paged", "Conversation", &["GetConversation"])
        .does(READ, SESSION),
    view("views.watch", "views::DetailViews::watch", "Changes in a scope as they happen (stream)", "ActivityEvent", &["WatchActivity"])
        .does(READ, VIEW_SCOPE),
];
