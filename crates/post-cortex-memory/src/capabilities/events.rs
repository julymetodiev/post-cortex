// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Events: the organisation's bus as memory keeps it (docs/host.md, events
//! of a module; docs/memory.md, section 4). The host stamps an event and
//! appends it here; a follower reads a name's events after its cursor and
//! acknowledges them; events older than [`EVENT_DAYS`](post_cortex_core::core::event::EVENT_DAYS) are dropped. Only
//! the host reaches it, through the composition: no tool, no client.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use post_cortex_core::core::event::{BusEvent, NewBusEvent, kept_since};
use post_cortex_ports::{EventCursors, EventLog, PortError};

/// Why an events operation failed.
#[derive(Debug, thiserror::Error)]
pub enum EventError {
    /// A read asked for no event.
    #[error("Invalid argument: {0}")]
    InvalidArgument(String),
    /// Storage failed.
    #[error("Internal error: {0}")]
    Storage(#[from] crate::memory_system::RuntimeError),
}

/// What a follower reads of one name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Followed {
    /// The events after its cursor, oldest first.
    pub events: Vec<BusEvent>,
    /// Events of the name it had not acknowledged were dropped: it re-reads
    /// what it follows.
    pub fell_behind: bool,
}

/// The Events capability service.
#[derive(Clone)]
pub struct Events {
    log: Arc<dyn EventLog>,
    cursors: Arc<dyn EventCursors>,
}

fn storage(e: PortError) -> EventError {
    EventError::Storage(e.into())
}

impl Events {
    /// The capability over the bus's ports.
    pub fn new(log: Arc<dyn EventLog>, cursors: Arc<dyn EventCursors>) -> Self {
        Self { log, cursors }
    }

    /// Keep `event` at the log's next position.
    ///
    /// # Errors
    /// Storage failed.
    pub async fn append(&self, event: NewBusEvent) -> Result<BusEvent, EventError> {
        self.log.append_event(event).await.map_err(storage)
    }

    /// At most `limit` events named `name` after `follower`'s cursor.
    ///
    /// # Errors
    /// `InvalidArgument` for a limit of 0; storage failed.
    pub async fn read(
        &self,
        follower: &str,
        name: &str,
        limit: usize,
    ) -> Result<Followed, EventError> {
        if limit == 0 {
            return Err(EventError::InvalidArgument(
                "a read of events asks for at least one".to_string(),
            ));
        }
        let cursor = self.cursors.cursor(follower, name).await.map_err(storage)?;
        let dropped = self.log.dropped_through(name).await.map_err(storage)?;
        let events = self
            .log
            .events_after(name, cursor, limit)
            .await
            .map_err(storage)?;
        Ok(Followed {
            events,
            // A follower that never acknowledged one is new, not behind.
            fell_behind: cursor > 0 && dropped > cursor,
        })
    }

    /// `follower` has handled the events named `name` up to `position`.
    ///
    /// # Errors
    /// Storage failed.
    pub async fn acknowledge(
        &self,
        follower: &str,
        name: &str,
        position: u64,
    ) -> Result<(), EventError> {
        self.cursors
            .advance_cursor(follower, name, position)
            .await
            .map_err(storage)
    }

    /// Drop the events older than the bus keeps them at `now`; how many.
    ///
    /// # Errors
    /// Storage failed.
    pub async fn drop_old(&self, now: DateTime<Utc>) -> Result<usize, EventError> {
        self.log
            .drop_events_before(kept_since(now))
            .await
            .map_err(storage)
    }
}

#[cfg(test)]
mod tests;
