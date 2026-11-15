// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The change feed of a memory instance (ARCHITECTURE section 4,
//! ChangeLog): every change storage holds, published once its write has
//! finished, so a subscriber never hears of something storage does not
//! hold. Records are published by the session persister; messages, cards,
//! documents and workspaces by their capabilities after their store writes.
//!
//! One bounded broadcast per instance: it never blocks a writer, and a
//! subscriber that falls more than [`CHANGE_CAPACITY`] changes behind gets
//! `RecvError::Lagged` and should re-read what it watches.

use chrono::{DateTime, Utc};
use post_cortex_core::core::card::CardScope;
use post_cortex_core::core::document::DocumentKind;
use tokio::sync::broadcast;
use tracing::debug;
use uuid::Uuid;

use crate::memory_system::RecordEvent;

/// Changes the feed buffers per subscriber.
pub const CHANGE_CAPACITY: usize = 1024;

/// What happened to a session's message stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessagesChangeKind {
    /// Turns were appended.
    Appended,
    /// Turns were deleted (one, the whole stream, or by retention).
    Deleted,
}

/// A change of a session's message stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessagesChange {
    /// The session.
    pub session_id: Uuid,
    /// Appended or deleted.
    pub kind: MessagesChangeKind,
    /// The positions appended or deleted, in order.
    pub seqs: Vec<u64>,
    /// When.
    pub at: DateTime<Utc>,
}

/// A new version of a card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CardChange {
    /// The card.
    pub scope: CardScope,
    /// Its version now.
    pub version: u64,
    /// When it was written.
    pub at: DateTime<Utc>,
}

/// A document version written, or a document deleted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocumentChange {
    /// Its workspace.
    pub workspace_id: Uuid,
    /// The document.
    pub id: Uuid,
    /// Its kind.
    pub kind: DocumentKind,
    /// Its version now; `None`: deleted.
    pub version: Option<u64>,
    /// When.
    pub at: DateTime<Utc>,
    /// The reach of the call that wrote it (docs/connect.md 4a); none:
    /// every workspace.
    pub reach: Option<Vec<Uuid>>,
}

/// What happened to a workspace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceChangeKind {
    /// Created.
    Created,
    /// Renamed or described anew.
    Updated,
    /// Archived.
    Archived,
    /// Unarchived.
    Unarchived,
    /// Deleted (its sessions stay).
    Deleted,
    /// A session joined it (or its role changed).
    SessionAdded(Uuid),
    /// A session left it.
    SessionRemoved(Uuid),
}

/// A change of a workspace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorkspaceChange {
    /// The workspace.
    pub workspace_id: Uuid,
    /// What happened.
    pub kind: WorkspaceChangeKind,
    /// When.
    pub at: DateTime<Utc>,
}

/// One change storage holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    /// A record written, forgotten or removed.
    Record(RecordEvent),
    /// Turns appended or deleted.
    Messages(MessagesChange),
    /// A card version written.
    Card(CardChange),
    /// A document version written or a document deleted.
    Document(DocumentChange),
    /// A workspace created, changed or deleted, or its membership.
    Workspace(WorkspaceChange),
}

impl Change {
    /// The record change, if this is one.
    pub fn record(&self) -> Option<&RecordEvent> {
        match self {
            Self::Record(event) => Some(event),
            Self::Messages(_) | Self::Card(_) | Self::Document(_) | Self::Workspace(_) => None,
        }
    }
}

/// The feed (see the module docs). Cheap to clone: every clone publishes to
/// the same subscribers.
#[derive(Clone)]
pub struct ChangeLog {
    tx: broadcast::Sender<Change>,
}

impl Default for ChangeLog {
    fn default() -> Self {
        Self::new()
    }
}

impl ChangeLog {
    /// A feed without subscribers.
    pub fn new() -> Self {
        Self {
            tx: broadcast::channel(CHANGE_CAPACITY).0,
        }
    }

    /// Whether anyone listens (publishers may skip building changes then).
    pub fn has_subscribers(&self) -> bool {
        self.tx.receiver_count() > 0
    }

    /// Tell every subscriber about `change`.
    pub fn publish(&self, change: Change) {
        // `send` fails only when nobody listens, which is not an error.
        if self.has_subscribers() && self.tx.send(change).is_err() {
            debug!("the last change subscriber left during a publish");
        }
    }

    /// A subscriber of every change published from now on.
    pub fn subscribe(&self) -> broadcast::Receiver<Change> {
        self.tx.subscribe()
    }
}
