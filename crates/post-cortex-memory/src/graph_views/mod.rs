// Copyright (c) 2026 Julius ML
// MIT License

//! Read-only views over a scope's knowledge graph and records, for agents:
//! an entity with its records, who
//! depends on it, the causal chain around it, topics, what needs attention,
//! and what the records say about a source file.
//!
//! The MCP `query_conversation_context` tool and the gRPC `GetGraphView`
//! RPC render the same views; the file view also backs `GET /api/file` for
//! the editor hook.
//!
//! A [`GraphScope`] is one session, or the sessions of a workspace with
//! their merged graph. Views read all of the scope's records, not only the
//! hot tier, and derive everything else from them: which records mention
//! an entity, which records assert an edge, and when each record and edge
//! held ([`post_cortex_core::graph::temporal`]).

use std::sync::Arc;

use post_cortex_core::graph::entity_graph::SimpleEntityGraph;
use post_cortex_core::session::active_session::ActiveSession;

pub mod detail;
mod entity_views;
mod file_view;
mod index;
mod radar;
mod record_files;
mod request;
mod types;

pub use index::Index;
pub use request::{DEFAULT_LIMIT, MAX_LIMIT, RenderedView, VIEW_NAMES, ViewError, ViewRequest};
pub use types::{
    AliasCandidate, EntityView, FileView, GraphEdge, GraphNode, ImpactItem, ImpactView,
    LineageLink, LineageLinks, LineageStep, LineageView, RadarView, RecordView, RelationRef,
    TopicView, TopicsView,
};

/// Characters of a record's description a view carries.
pub const DESCRIPTION_CHARS: usize = 400;
/// Entities untouched this long are reported as stale by the radar.
pub const STALE_AFTER_DAYS: i64 = 90;

/// The sessions a view reads and the graph over them.
pub struct GraphScope {
    /// Sessions in the scope (one, or a workspace's members).
    pub sessions: Vec<Arc<ActiveSession>>,
    /// The session's graph, or the members' graphs merged.
    pub graph: Arc<SimpleEntityGraph>,
}

#[cfg(test)]
mod record_files_tests;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;
