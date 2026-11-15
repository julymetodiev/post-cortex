// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The bus's ports (docs/host.md, events of a module): the tenant's one
//! ordered log of events and each follower's acknowledged positions.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use post_cortex_core::core::event::{BusEvent, NewBusEvent};

use crate::error::Result;

/// The ordered log of the organisation's events.
#[async_trait]
pub trait EventLog: Send + Sync {
    /// Append `event` at the next position, never one used before (even
    /// after the events before it were dropped); two appends at once get
    /// two positions.
    async fn append_event(&self, event: NewBusEvent) -> Result<BusEvent>;

    /// The events named `name` after position `after`, oldest first, at
    /// most `limit`.
    async fn events_after(&self, name: &str, after: u64, limit: usize) -> Result<Vec<BusEvent>>;

    /// The position of the oldest event kept; none when the log is empty.
    async fn oldest_event(&self) -> Result<Option<u64>>;

    /// Drop the events emitted before `before`; how many were dropped.
    async fn drop_events_before(&self, before: DateTime<Utc>) -> Result<usize>;

    /// The newest position of an event named `name` ever dropped; 0: none.
    async fn dropped_through(&self, name: &str) -> Result<u64>;
}

/// The position each follower acknowledged, per event name.
#[async_trait]
pub trait EventCursors: Send + Sync {
    /// The position `follower` acknowledged of the events named `name`; 0:
    /// none yet.
    async fn cursor(&self, follower: &str, name: &str) -> Result<u64>;

    /// Move `follower`'s cursor of `name` to `position`; a position at or
    /// before the kept one leaves it.
    async fn advance_cursor(&self, follower: &str, name: &str, position: u64) -> Result<()>;
}
