// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Which records of a session have their embedding in the vector index.
//!
//! The write path returns once a record is committed; the embedding
//! pipeline adds its vector shortly after. In between the record is
//! pending: in the session, not yet marked here. The set belongs to the
//! session's cache entry ([`crate::memory_system::session_cache::CachedSession`]), so every
//! version of the session shares it (the pipeline marks records without
//! publishing a new version), and it is stored next to the session
//! (each save carries the ids marked and unmarked since the last one, see
//! [`VectorizedIds::take_unsaved`]), so the missing-record pass after a
//! restart can read it.

use dashmap::DashSet;
use parking_lot::Mutex;
use post_cortex_core::session::active_session::ActiveSession;
use uuid::Uuid;

/// The ids of a session's embedded records (see the module docs).
#[derive(Debug, Default)]
pub struct VectorizedIds {
    ids: DashSet<Uuid>,
    /// Marks and unmarks no save has carried yet.
    unsaved: Mutex<Unsaved>,
}

/// Ids marked and unmarked since the last save took them.
#[derive(Debug, Default)]
struct Unsaved {
    marked: Vec<Uuid>,
    unmarked: Vec<Uuid>,
}

impl VectorizedIds {
    /// A set holding `ids` (as stored with the session).
    pub fn new(ids: impl IntoIterator<Item = Uuid>) -> Self {
        Self {
            ids: ids.into_iter().collect(),
            unsaved: Mutex::default(),
        }
    }

    /// Whether the record `id` has its embedding in the vector index.
    pub fn contains(&self, id: &Uuid) -> bool {
        self.ids.contains(id)
    }

    /// Mark the record `id` as embedded.
    pub fn mark(&self, id: Uuid) {
        if self.ids.insert(id) {
            let mut unsaved = self.unsaved.lock();
            unsaved.unmarked.retain(|u| *u != id);
            unsaved.marked.push(id);
        }
    }

    /// Mark the record `id` as not embedded.
    pub fn unmark(&self, id: &Uuid) {
        if self.ids.remove(id).is_some() {
            let mut unsaved = self.unsaved.lock();
            unsaved.marked.retain(|u| u != id);
            unsaved.unmarked.push(*id);
        }
    }

    /// The ids marked and the ids unmarked since the last call, for the
    /// save that stores them (a failed save carries them to its retry).
    pub fn take_unsaved(&self) -> (Vec<Uuid>, Vec<Uuid>) {
        let unsaved = std::mem::take(&mut *self.unsaved.lock());
        (unsaved.marked, unsaved.unmarked)
    }

    /// Ids of the embedded records, in no particular order.
    pub fn ids(&self) -> Vec<Uuid> {
        self.ids.iter().map(|id| *id).collect()
    }

    /// How many records are marked as embedded.
    pub fn len(&self) -> usize {
        self.ids.len()
    }

    /// Whether no record is marked.
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// How many records of `session` are waiting for their embedding.
    pub fn pending_count(&self, session: &ActiveSession) -> usize {
        session
            .records()
            .iter()
            .filter(|u| !self.contains(&u.id))
            .count()
    }
}

#[cfg(test)]
mod tests;
