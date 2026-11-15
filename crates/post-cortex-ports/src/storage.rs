// Copyright (c) 2025, 2026 Julius ML
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

//! The storage ports, one trait per capability (ARCHITECTURE section 5).
//!
//! A backend is one struct implementing the ports it supports;
//! [`Stores`] hands each capability its `Arc<dyn Port>`, so a consumer
//! depends only on the ports it uses. Every port has a conformance suite
//! (`crate::conformance`, feature `conformance`) run against every
//! adapter, from the adapter crate's tests.

use chrono::{DateTime, Utc};
use post_cortex_core::session::active_session::ActiveSession;
use uuid::Uuid;

mod cards;
mod checkpoints;
mod dismissals;
mod documents;
mod events;
mod freshness;
mod messages;
mod sessions;
mod stores;
mod vectors;
mod workspaces;

pub use cards::{CardStore, CardWrite};
pub use checkpoints::{CheckpointStore, SessionCheckpoint};
pub use dismissals::DismissalStore;
pub use documents::{DocumentStore, DocumentWrite, LatestPage, LatestQuery};
pub use events::{EventCursors, EventLog};
pub use freshness::{FreshnessStore, InvalidationScope, StaleEntryInfo};
pub use messages::MessageStore;
pub use sessions::{SessionChanges, SessionStore};
pub use stores::Stores;
pub use vectors::VectorStore;
pub use workspaces::{StoredWorkspace, WorkspaceStore, WorkspaceUpdate};

/// Listing row for a stored session: its metadata without the context.
///
/// Returned by [`SessionStore::list_session_summaries`] so callers can list and
/// filter sessions without loading (and deserialising) every session.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SessionSummary {
    /// Session ID.
    pub id: Uuid,
    /// Display name, if set.
    #[serde(default)]
    pub name: Option<String>,
    /// Description, if set.
    #[serde(default)]
    pub description: Option<String>,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Last modification timestamp as of the last save.
    pub last_updated: DateTime<Utc>,
    /// Number of context updates (`incremental_updates`) in the session.
    #[serde(default)]
    pub update_count: usize,
}

impl SessionSummary {
    /// Summary of an in-memory session.
    pub fn from_session(session: &ActiveSession) -> Self {
        let meta = session.metadata();
        Self {
            id: meta.id,
            name: meta.name.clone(),
            description: meta.description.clone(),
            created_at: meta.created_at,
            last_updated: session.last_updated(),
            update_count: session.records().len(),
        }
    }
}
