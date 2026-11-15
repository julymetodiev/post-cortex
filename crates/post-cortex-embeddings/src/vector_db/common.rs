// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Ranked candidates and the bounded top-k collector every ranking (vector
//! and BM25) uses.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

/// A row's place in a ranking.
#[derive(Debug, Clone, Copy)]
pub struct SearchResult {
    /// The arena slot.
    pub slot: u32,
    /// Similarity or BM25 score.
    pub score: f32,
    /// The row's [`tie key`](crate::vector_db::types::tie_key): equal scores
    /// rank by what the content is, never by where it is stored.
    pub key: (u64, u64),
}

impl PartialEq for SearchResult {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == std::cmp::Ordering::Equal
    }
}

impl Eq for SearchResult {}

impl PartialOrd for SearchResult {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SearchResult {
    /// Higher score is greater (a max-heap pops the best first); ties go
    /// to the lower key, then the lower slot. `total_cmp` keeps the order
    /// total even if a NaN slips in.
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.score
            .total_cmp(&other.score)
            .then_with(|| other.key.cmp(&self.key))
            .then_with(|| other.slot.cmp(&self.slot))
    }
}

/// Bounded top-k collector: O(n log k) instead of sorting all n scores.
pub struct TopK {
    k: usize,
    heap: BinaryHeap<Reverse<SearchResult>>,
}

impl TopK {
    pub fn new(k: usize) -> Self {
        Self {
            k,
            heap: BinaryHeap::with_capacity(k.min(1024) + 1),
        }
    }

    /// Offer a candidate; NaN scores are dropped.
    #[inline]
    pub fn push(&mut self, candidate: SearchResult) {
        if self.k == 0 || candidate.score.is_nan() {
            return;
        }
        if self.heap.len() < self.k {
            self.heap.push(Reverse(candidate));
        } else if let Some(Reverse(worst)) = self.heap.peek()
            && candidate > *worst
        {
            self.heap.pop();
            self.heap.push(Reverse(candidate));
        }
    }

    /// The best `k` of both collectors.
    pub fn merge(mut self, other: Self) -> Self {
        for Reverse(candidate) in other.heap {
            self.push(candidate);
        }
        self
    }

    /// Results sorted best first.
    pub fn into_sorted(self) -> Vec<SearchResult> {
        // Ascending order of `Reverse` is descending order of the score.
        self.heap
            .into_sorted_vec()
            .into_iter()
            .map(|r| r.0)
            .collect()
    }
}

#[cfg(test)]
mod tests;
