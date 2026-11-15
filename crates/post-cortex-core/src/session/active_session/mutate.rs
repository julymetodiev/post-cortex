// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The changes of the record list: append, remove, retire.

use crate::core::context_update::ContextUpdate;
use crate::graph::entity_graph::{SimpleEntityGraph, mentioned_names};
use crate::session::active_session::{ActiveSession, normalised};

use chrono::{DateTime, Utc};
use std::collections::HashSet;
use std::sync::Arc;
use uuid::Uuid;

impl ActiveSession {
    /// Apply one record: normalise it (see [`normalised`]), merge what it
    /// says into the entity graph and append it. The session's
    /// `last_updated` becomes the record's time if that is later. Returns
    /// the record as stored.
    pub fn apply_record(&mut self, update: ContextUpdate) -> Arc<ContextUpdate> {
        let shared = Arc::new(normalised(update));
        self.apply_normalised_record(Arc::clone(&shared));
        shared
    }

    /// [`Self::apply_record`] for a record already passed through
    /// [`normalised`], shared so a retried compare-and-swap does not copy
    /// it.
    pub fn apply_normalised_record(&mut self, update: Arc<ContextUpdate>) {
        if mentioned_names(&update).next().is_some() {
            Arc::make_mut(&mut self.entity_graph).apply_update(&update);
        }
        self.touch(update.timestamp);
        self.records.push(update);
    }

    /// Replace every record (oldest first), e.g. with the rows a backend
    /// stores apart from the session; the graph's record links are rebuilt.
    pub fn replace_records(&mut self, records: Vec<ContextUpdate>) {
        self.records = records.into_iter().map(Arc::new).collect();
        let records = self.records.clone();
        Arc::make_mut(&mut self.entity_graph).relink_mentions(records.iter().map(|u| &**u));
    }

    /// Remove the records with these ids and their links from the entity
    /// graph. Everything else about a record is derived from the list, so
    /// no view refers to a removed record afterwards. Returns the ids that
    /// were removed, in record order.
    pub fn remove_records(&mut self, ids: &[Uuid]) -> Vec<Uuid> {
        let wanted: HashSet<Uuid> = ids.iter().copied().collect();
        let removed: Vec<Uuid> = self
            .records
            .iter()
            .map(|u| u.id)
            .filter(|id| wanted.contains(id))
            .collect();
        if removed.is_empty() {
            return removed;
        }
        let gone: HashSet<Uuid> = removed.iter().copied().collect();
        self.records.retain(|u| !gone.contains(&u.id));
        let linked: Vec<String> = self
            .entity_graph
            .entity_mentions
            .iter_unordered()
            .filter(|(_, links)| links.iter().any(|id| gone.contains(id)))
            .map(|(name, _)| name.clone())
            .collect();
        if !linked.is_empty() {
            let graph = Arc::make_mut(&mut self.entity_graph);
            for name in linked {
                if let Some(links) = graph.entity_mentions.get_mut(&name) {
                    links.retain(|id| !gone.contains(id));
                }
            }
            graph.entity_mentions.retain(|_, links| !links.is_empty());
        }
        removed
    }

    /// Remove every record whose code reference is in `file_path` (see
    /// [`remove_records`](Self::remove_records)). Returns their ids, oldest
    /// first.
    pub fn remove_updates_for_file(&mut self, file_path: &str) -> Vec<Uuid> {
        let ids: Vec<Uuid> = self
            .records
            .iter()
            .filter(|u| {
                u.related_code
                    .as_ref()
                    .is_some_and(|code| code.file_path == file_path)
            })
            .map(|u| u.id)
            .collect();
        self.remove_records(&ids)
    }

    /// Retire a record at `at` (`forget`): it stays in the history but
    /// stops being valid then. Returns the changed record (for the storage
    /// write), or `None` if it is unknown or already retired.
    pub fn retire_update(&mut self, entry_id: &Uuid, at: DateTime<Utc>) -> Option<ContextUpdate> {
        let pos = self.records.iter().position(|u| u.id == *entry_id)?;
        if self.records[pos].meta.retired_at.is_some() {
            return None;
        }
        let mut changed = ContextUpdate::clone(&self.records[pos]);
        changed.meta.retired_at = Some(at);
        if let Some(slot) = self.records.get_mut(pos) {
            *slot = Arc::new(changed.clone());
        }
        Some(changed)
    }

    /// Rebuild the entity graph from the records alone, e.g. after a
    /// purge, so nothing derived from a removed record survives. Explicit
    /// merges (aliases that do not resolve by name alone) are kept.
    pub fn rebuild_entity_graph(&mut self) {
        let merges = self.entity_graph.explicit_aliases();
        let mut graph = SimpleEntityGraph::new();
        for update in self.records.iter() {
            graph.apply_update(update);
        }
        for (alias, into) in merges {
            graph.merge_entities(&alias, &into);
        }
        self.entity_graph = Arc::new(graph);
    }
}
