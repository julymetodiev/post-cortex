// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Record changes: what the session persister publishes to the change log
//! ([`crate::memory_system::change_log`]) once a storage write that carries
//! them has finished, so a subscriber never hears of a record storage does
//! not hold yet. The record itself is already in the session in memory by
//! then (writes commit there first), so a watcher can read it back through
//! the session.

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use post_cortex_core::core::context_update::{ContextUpdate, UpdateType};

/// What happened to a record.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RecordEventKind {
    /// Written: a new record (or a row storage lacked) is stored.
    Written,
    /// Retired (`forget`): its validity ended, it stays in the history.
    Forgotten,
    /// Its row was deleted (`purge`, `delete_update`, a source
    /// invalidation). A record removed before its first write was stored
    /// is announced as removed without ever being announced as written.
    Removed,
}

/// A record change that storage holds.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecordEvent {
    /// Session the record belongs to.
    pub session_id: Uuid,
    /// The record's entry id.
    pub entry_id: Uuid,
    /// When the change happened: the record's timestamp when written, its
    /// retirement time when forgotten, the write's time when removed.
    pub timestamp: DateTime<Utc>,
    /// What happened.
    pub kind: RecordEventKind,
    /// The record's type; none for a removed record, whose row is gone.
    #[serde(default)]
    pub update_type: Option<UpdateType>,
}

/// Events for one storage write of `session_id` carrying `updates` and
/// deleting `removed`, one per record and kind.
pub(crate) fn events_of_write(
    session_id: Uuid,
    updates: &[ContextUpdate],
    removed: &[Uuid],
) -> Vec<RecordEvent> {
    let mut seen = HashSet::new();
    let mut events = Vec::new();
    for update in updates {
        let (kind, timestamp) = match update.meta.retired_at {
            Some(at) => (RecordEventKind::Forgotten, at),
            None => (RecordEventKind::Written, update.timestamp),
        };
        if seen.insert((update.id, kind)) {
            events.push(RecordEvent {
                session_id,
                entry_id: update.id,
                timestamp,
                kind,
                update_type: Some(update.update_type),
            });
        }
    }
    let now = Utc::now();
    for &entry_id in removed {
        if seen.insert((entry_id, RecordEventKind::Removed)) {
            events.push(RecordEvent {
                session_id,
                entry_id,
                timestamp: now,
                kind: RecordEventKind::Removed,
                update_type: None,
            });
        }
    }
    events
}
