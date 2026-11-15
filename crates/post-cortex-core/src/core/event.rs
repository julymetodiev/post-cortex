// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The organisation's bus (docs/host.md, events of a module): the events
//! modules emit, kept in one ordered log of the tenant's store, and the
//! position each follower acknowledged.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// How long an event is kept, in days.
pub const EVENT_DAYS: i64 = 7;

/// An event the host stamped, before the log gives it its position.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct NewBusEvent {
    /// Its id (a UUIDv7), by which a follower deduplicates.
    pub id: Uuid,
    /// Its name, `<module id>.<name>` (`board.task_moved`).
    pub name: String,
    /// The module that emitted it.
    pub module: String,
    /// The workspace it is of, when it is of one.
    pub workspace_id: Option<Uuid>,
    /// Who acted, when known.
    pub author: Option<String>,
    /// When it was emitted.
    pub at: DateTime<Utc>,
    /// The full name of its message's type.
    pub payload_type: String,
    /// Its message, encoded.
    pub payload: Vec<u8>,
}

/// An event of the log.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct BusEvent {
    /// Its position in the log: 1 for the first, one more for each after;
    /// never reused.
    pub position: u64,
    /// The event.
    pub event: NewBusEvent,
}

/// The oldest time an event is kept at `now`: one emitted before it is
/// dropped.
#[must_use]
pub fn kept_since(now: DateTime<Utc>) -> DateTime<Utc> {
    now - chrono::Duration::days(EVENT_DAYS)
}
