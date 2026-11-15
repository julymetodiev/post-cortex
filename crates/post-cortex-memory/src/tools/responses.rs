// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! One serde response shape per operation (ARCHITECTURE section 6).
//!
//! Every transport that carries an operation's response as JSON (the MCP
//! `Results:` block, REST bodies) serialises this one type, and the MCP
//! tools' `select` runs over its serde form. Where a
//! capability already returns a serde type (summaries, views, cards, the
//! project resolution) that type is the shape; the types here shape the
//! rest. Times are RFC 3339 strings as the tools have always written them.

mod admin;
mod assembly;
mod documents;
mod entities;
mod messages;
mod records;
mod search;
mod sessions;
mod views;
mod workspaces;

pub use admin::{Checkpoint, HealthPayload, Vectorized};
pub use assembly::Assembled;
pub use documents::{
    DocumentDeleted, DocumentHistory, DocumentJson, DocumentList, DocumentVersion, DocumentWritten,
};
pub use entities::{EntitiesMerged, EntityDeleted};
pub use messages::{MessagePage, MessagesAppended};
pub use records::{BulkItem, BulkWritten, DryRun, RecordChanged, RecordWhole, RecordWritten};
pub use search::{RelatedContent, RelatedHit, SearchHit, SearchResponse};
pub use sessions::{
    MessagesDeleted, SessionCreated, SessionDeleted, SessionInfo, SessionList, SessionListing,
    SessionSearch, SessionUpdated,
};
pub use views::{SummaryReply, SummarySections};
pub use workspaces::{
    MemberAdded, MemberRemoved, RadarDismissals, RadarRestored, WorkspaceCreated, WorkspaceDeleted,
    WorkspaceDetail, WorkspaceList, WorkspaceListing, WorkspaceMember, WorkspaceSummary,
};

/// `t` as the tools write times.
fn rfc3339(t: chrono::DateTime<chrono::Utc>) -> String {
    t.to_rfc3339()
}
