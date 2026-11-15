// Copyright (c) 2026 Julius ML
// MIT License

//! Merged entity graphs of workspaces (and of every session), per memory
//! instance.
//!
//! A workspace's graph is its member sessions' graphs merged; the global
//! graph is every session's. The merge is
//! cached and reused for as long as every member still points at the same
//! graph `Arc`; holding the source `Arc`s makes that pointer comparison
//! sound (their addresses cannot be reused while held, and `Arc::make_mut`
//! on a session's graph copies instead of mutating in place). The cache
//! holds at most [`CAPACITY`] workspaces (least recently used out first)
//! and drops a workspace on delete and on membership change.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use dashmap::DashMap;
use post_cortex_core::graph::entity_graph::SimpleEntityGraph;
use uuid::Uuid;

/// Workspaces whose merged graph is kept.
pub(crate) const CAPACITY: usize = 64;

/// Which merge a cache entry holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MergeKey {
    /// The members of a workspace.
    Workspace(Uuid),
    /// Every session.
    Global,
}

struct CachedMerge {
    sources: Vec<(Uuid, Arc<SimpleEntityGraph>)>,
    merged: Arc<SimpleEntityGraph>,
    /// Tick of the last use, for eviction.
    used: AtomicU64,
}

/// The cache (see the module docs).
#[derive(Default)]
pub struct WorkspaceGraphCache {
    entries: DashMap<MergeKey, CachedMerge>,
    clock: AtomicU64,
}

impl WorkspaceGraphCache {
    /// The merged graph of `sources` (the sessions of `key`), rebuilt only
    /// when the set of (session id, graph `Arc`) pairs changed since the
    /// last call.
    pub(crate) fn merged(
        &self,
        key: MergeKey,
        mut sources: Vec<(Uuid, Arc<SimpleEntityGraph>)>,
    ) -> Arc<SimpleEntityGraph> {
        // Membership is a map, so its order is not stable; sort so both the
        // cache key and the merge order are.
        sources.sort_unstable_by_key(|(id, _)| *id);
        let tick = self.clock.fetch_add(1, Ordering::Relaxed);

        if let Some(hit) = self.entries.get(&key)
            && hit.sources.len() == sources.len()
            && hit
                .sources
                .iter()
                .zip(&sources)
                .all(|((a_id, a), (b_id, b))| a_id == b_id && Arc::ptr_eq(a, b))
        {
            hit.used.store(tick, Ordering::Relaxed);
            return Arc::clone(&hit.merged);
        }

        let mut merged = SimpleEntityGraph::new();
        for (_, graph) in &sources {
            merged.merge_from(graph);
        }
        let merged = Arc::new(merged);
        self.entries.insert(
            key,
            CachedMerge {
                sources,
                merged: Arc::clone(&merged),
                used: AtomicU64::new(tick),
            },
        );
        self.evict_over_capacity();
        merged
    }

    /// Forget the merged graph of `workspace_id`.
    pub(crate) fn invalidate(&self, workspace_id: Uuid) {
        self.entries.remove(&MergeKey::Workspace(workspace_id));
    }

    /// How many workspaces are cached.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    fn evict_over_capacity(&self) {
        while self.entries.len() > CAPACITY {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|entry| entry.used.load(Ordering::Relaxed))
                .map(|entry| *entry.key());
            match oldest {
                Some(key) => {
                    self.entries.remove(&key);
                }
                None => break,
            }
        }
    }
}

#[cfg(test)]
mod tests;
