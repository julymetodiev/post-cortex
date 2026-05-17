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

    /// Remove vector from index with atomic CAS loop
    pub(super) fn remove_vector(&self, vector_id: u32) {
        self.layers.remove(&vector_id);
        self.connections.remove(&vector_id);

        // Remove connections to this vector from other vectors
        for mut entry in self.connections.iter_mut() {
            let connections = entry.value_mut();
            connections.retain(|&id| id != vector_id);
        }

        // CAS loop to atomically update entry point if we removed it
        loop {
            let current = self.entry_state.load(Ordering::Acquire);
            let (current_ep, _) = Self::unpack_state(current);

            // Only update if this vector was the entry point
            if current_ep != vector_id {
                break;
            }

            // Find new entry point from remaining vectors
            let mut new_max_layer: u32 = 0;
            let mut new_entry_point: u32 = u32::MAX;

            for entry in self.layers.iter() {
                let layer = *entry.value() as u32;
                if layer >= new_max_layer {
                    new_max_layer = layer;
                    new_entry_point = *entry.key();
                }
            }

            let new_state = Self::pack_state(new_entry_point, new_max_layer);

            match self.entry_state.compare_exchange(
                current,
                new_state,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(_) => continue, // CAS failed, another thread modified - retry
            }
        }
    }

    /// Get connections for a vector
    pub(super) fn get_connections(&self, vector_id: u32) -> Option<Vec<u32>> {
        self.connections.get(&vector_id).map(|entry| entry.clone())
    }
}
