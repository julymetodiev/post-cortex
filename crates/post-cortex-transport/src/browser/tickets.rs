// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Single-use download links (an export, a blob): a navigation carries no
//! `Authorization`, so the link is its credential: random, short-lived,
//! used once, kept in this process.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tonic::Status;
use uuid::Uuid;

/// Most pending links of one kind (a full table refuses new links).
const MAX_PENDING: usize = 1_000;

/// The pending links of one kind, each holding what it downloads.
pub struct Tickets<T> {
    pending: Mutex<HashMap<String, (T, Instant)>>,
    ttl: Duration,
}

impl<T> Tickets<T> {
    /// Links valid for `ttl`.
    pub fn new(ttl: Duration) -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
            ttl,
        }
    }

    /// How long a link is valid.
    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    /// A new link's ticket for `what`.
    ///
    /// # Errors
    /// `Unavailable` while too many links are pending.
    pub fn issue(&self, what: T) -> Result<String, Status> {
        let ticket = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let mut pending = self.pending.lock();
        let now = Instant::now();
        pending.retain(|_, (_, expires)| *expires > now);
        if pending.len() >= MAX_PENDING {
            return Err(Status::unavailable(
                "too many pending downloads, retry shortly",
            ));
        }
        pending.insert(ticket.clone(), (what, now + self.ttl));
        Ok(ticket)
    }

    /// What `ticket` downloads, once; none when it expired or was used.
    pub fn take(&self, ticket: &str) -> Option<T> {
        self.pending
            .lock()
            .remove(ticket)
            .filter(|(_, expires)| *expires > Instant::now())
            .map(|(what, _)| what)
    }
}
