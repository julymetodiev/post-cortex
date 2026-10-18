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

//! HNSW index using DashMap + atomic entry-state CAS.

use dashmap::DashMap;
use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};

/// HNSW index using DashMap for concurrent access
///
/// Uses a combined AtomicU64 for entry_point and max_layer to enable
/// atomic compare-and-swap updates, preventing race conditions.
#[derive(Debug, Default)]
pub(super) struct HnswIndex {
    /// Graph connections for each vector
    pub(super) connections: DashMap<u32, Vec<u32>>,
    /// Combined entry point and max layer state (atomic)
    /// Upper 32 bits = entry_point (u32::MAX = no entry point)
    /// Lower 32 bits = max_layer
    entry_state: AtomicU64,
    /// Layer assignments for each vector
    layers: DashMap<u32, usize>,
}

impl HnswIndex {
    /// Pack entry point and max layer into a single u64
    /// Upper 32 bits = entry_point, Lower 32 bits = max_layer
    #[inline]
    fn pack_state(entry_point: u32, max_layer: u32) -> u64 {
        ((entry_point as u64) << 32) | (max_layer as u64)
    }

    /// Unpack u64 into (entry_point, max_layer)
    #[inline]
    fn unpack_state(state: u64) -> (u32, u32) {
        let entry_point = (state >> 32) as u32;
        let max_layer = (state & 0xFFFFFFFF) as u32;
        (entry_point, max_layer)
    }

    pub(super) fn new() -> Self {
        Self {
            connections: DashMap::new(),
            // Initialize with no entry point (u32::MAX) and max_layer = 0
            entry_state: AtomicU64::new(Self::pack_state(u32::MAX, 0)),
            layers: DashMap::new(),
        }
    }

    /// Check if index is empty
    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        let (entry_point, _) = Self::unpack_state(self.entry_state.load(Ordering::Acquire));
        entry_point == u32::MAX
    }

    /// Get entry point
    pub(super) fn get_entry_point(&self) -> Option<u32> {
        let (entry_point, _) = Self::unpack_state(self.entry_state.load(Ordering::Acquire));
        if entry_point == u32::MAX {
            None
        } else {
            Some(entry_point)
        }
    }

    /// Add vector to index with atomic CAS loop to prevent race conditions
    pub(super) fn add_vector(&self, vector_id: u32, layer: usize, connections: Vec<u32>) {
        self.layers.insert(vector_id, layer);
        self.connections.insert(vector_id, connections);

        // CAS loop to atomically update entry point if this is higher/equal layer
        loop {
            let current = self.entry_state.load(Ordering::Acquire);
            let (current_ep, current_max) = Self::unpack_state(current);

            // Update entry point if: empty OR this layer is >= max layer
            let should_update = current_ep == u32::MAX || layer as u32 >= current_max;
            if !should_update {
                break;
            }

            let new_max = current_max.max(layer as u32);
            let new_state = Self::pack_state(vector_id, new_max);

            match self.entry_state.compare_exchange(
                current,
                new_state,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(_) => continue, // CAS failed, retry
            }
        }
    }

    /// Remove vector from index.
    ///
    /// O(n): edges are not symmetric after pruning, so every adjacency list
    /// has to be checked for back-references. Use [`Self::remove_vectors`]
    /// to drop many ids in one pass.
    pub(super) fn remove_vector(&self, vector_id: u32) {
        self.layers.remove(&vector_id);
        self.connections.remove(&vector_id);

        // Remove connections to this vector from other vectors
        for mut entry in self.connections.iter_mut() {
            entry.value_mut().retain(|&id| id != vector_id);
        }

        self.reselect_entry_point();
    }

    /// Remove several vectors with a single pass over the adjacency lists.
    pub(super) fn remove_vectors(&self, vector_ids: &HashSet<u32>) {
        if vector_ids.is_empty() {
            return;
        }
        for id in vector_ids {
            self.layers.remove(id);
            self.connections.remove(id);
        }
        for mut entry in self.connections.iter_mut() {
            entry.value_mut().retain(|id| !vector_ids.contains(id));
        }
        self.reselect_entry_point();
    }

    /// Drop every node and reset the entry point. O(n), unlike removing
    /// nodes one by one (O(n²)).
    pub(super) fn clear(&self) {
        self.connections.clear();
        self.layers.clear();
        self.entry_state
            .store(Self::pack_state(u32::MAX, 0), Ordering::Release);
    }

    /// Replace the entry point if it no longer names a live node.
    ///
    /// Checking liveness (rather than "is it the id I just removed") also
    /// heals the race where two concurrent removals each pick the other's
    /// node: whichever CAS lands last re-validates and retries, so the entry
    /// point never stays pointed at a removed vector.
    fn reselect_entry_point(&self) {
        loop {
            let current = self.entry_state.load(Ordering::Acquire);
            let (current_ep, _) = Self::unpack_state(current);

            if current_ep == u32::MAX {
                // Empty index, or a node was inserted without updating the
                // entry state yet; adopt any live node if one exists.
                if self.layers.is_empty() {
                    return;
                }
            } else if self.layers.contains_key(&current_ep) {
                return;
            }

            // Highest layer wins; ties go to the lowest id for determinism.
            let mut best: Option<(u32, u32)> = None;
            for entry in self.layers.iter() {
                let candidate = (*entry.value() as u32, *entry.key());
                best = match best {
                    Some((layer, id))
                        if layer > candidate.0 || (layer == candidate.0 && id < candidate.1) =>
                    {
                        Some((layer, id))
                    }
                    _ => Some(candidate),
                };
            }
            let new_state = match best {
                Some((layer, id)) => Self::pack_state(id, layer),
                None => Self::pack_state(u32::MAX, 0),
            };

            if self
                .entry_state
                .compare_exchange(current, new_state, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                // Loop once more: the chosen node may have been removed
                // between the scan and the CAS.
                continue;
            }
        }
    }

    /// Number of indexed nodes.
    pub(super) fn len(&self) -> usize {
        self.layers.len()
    }

    /// Whether `vector_id` is an indexed node.
    pub(super) fn contains(&self, vector_id: u32) -> bool {
        self.layers.contains_key(&vector_id)
    }

    /// Ids of all indexed nodes.
    pub(super) fn node_ids(&self) -> Vec<u32> {
        self.layers.iter().map(|e| *e.key()).collect()
    }

    /// Copy `vector_id`'s neighbours into `out` (cleared first) without
    /// allocating a fresh `Vec` per visited node.
    pub(super) fn connections_into(&self, vector_id: u32, out: &mut Vec<u32>) -> bool {
        out.clear();
        match self.connections.get(&vector_id) {
            Some(entry) => {
                out.extend_from_slice(entry.value());
                true
            }
            None => false,
        }
    }

    /// Get connections for a vector
    #[cfg(test)]
    pub(super) fn get_connections(&self, vector_id: u32) -> Option<Vec<u32>> {
        self.connections.get(&vector_id).map(|entry| entry.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removing_the_entry_point_promotes_the_highest_layer_node() {
        let idx = HnswIndex::new();
        idx.add_vector(1, 0, vec![]);
        idx.add_vector(2, 2, vec![1]);
        idx.add_vector(3, 1, vec![2]);
        assert_eq!(idx.get_entry_point(), Some(2));

        idx.remove_vector(2);
        assert_eq!(idx.get_entry_point(), Some(3));
        assert_eq!(idx.get_connections(3), Some(vec![]));

        idx.remove_vector(3);
        assert_eq!(idx.get_entry_point(), Some(1));
        idx.remove_vector(1);
        assert!(idx.is_empty());
        assert_eq!(idx.len(), 0);
    }

    #[test]
    fn removing_a_non_entry_node_keeps_the_entry_point() {
        let idx = HnswIndex::new();
        idx.add_vector(1, 3, vec![]);
        idx.add_vector(2, 0, vec![1]);
        idx.remove_vector(2);
        assert_eq!(idx.get_entry_point(), Some(1));
    }

    #[test]
    fn stale_entry_point_is_healed() {
        let idx = HnswIndex::new();
        idx.add_vector(1, 0, vec![]);
        idx.add_vector(2, 0, vec![]);
        // Simulate the lost race: the entry point names a node that is gone.
        idx.layers.remove(&2);
        idx.connections.remove(&2);
        idx.reselect_entry_point();
        assert_eq!(idx.get_entry_point(), Some(1));
    }

    #[test]
    fn batch_remove_and_clear() {
        let idx = HnswIndex::new();
        for id in 0..10 {
            idx.add_vector(id, 0, (0..id).collect());
        }
        idx.remove_vectors(&(0..5).collect());
        assert_eq!(idx.len(), 5);
        assert!(idx.get_entry_point().is_some_and(|ep| ep >= 5));
        let mut buf = Vec::new();
        assert!(idx.connections_into(9, &mut buf));
        assert_eq!(buf, vec![5, 6, 7, 8]);

        idx.clear();
        assert!(idx.is_empty());
        assert!(!idx.contains(9));
    }

    #[test]
    fn concurrent_removals_never_leave_a_dead_entry_point() {
        use std::sync::Arc;
        for _ in 0..50 {
            let idx = Arc::new(HnswIndex::new());
            for id in 0..64 {
                idx.add_vector(id, (id % 3) as usize, vec![]);
            }
            let handles: Vec<_> = (0..4)
                .map(|t| {
                    let idx = Arc::clone(&idx);
                    std::thread::spawn(move || {
                        for id in (t..60).step_by(4) {
                            idx.remove_vector(id);
                        }
                    })
                })
                .collect();
            for h in handles {
                h.join().unwrap();
            }
            let ep = idx.get_entry_point().expect("4 nodes remain");
            assert!(idx.contains(ep), "entry point {ep} was removed");
        }
    }
}
