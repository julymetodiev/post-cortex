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

//! Shared internal helpers: `SearchResult` ordering, bounded top-k, Euclidean distance.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

/// Internal search result with ordering support for BinaryHeap
#[derive(Debug, Clone)]
pub(super) struct SearchResult {
    pub(super) id: u32,
    pub(super) similarity: f32,
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
    /// Higher similarity is greater (max-heap pops the best first). Ties go
    /// to the lower id so result order is deterministic. `total_cmp` keeps
    /// the order total even if a NaN slips in.
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.similarity
            .total_cmp(&other.similarity)
            .then_with(|| other.id.cmp(&self.id))
    }
}

/// Bounded top-k collector: O(n log k) instead of sorting all n scores.
pub(super) struct TopK {
    k: usize,
    heap: BinaryHeap<Reverse<SearchResult>>,
}

impl TopK {
    pub(super) fn new(k: usize) -> Self {
        Self {
            k,
            heap: BinaryHeap::with_capacity(k.min(1024) + 1),
        }
    }

    /// Offer a candidate; NaN scores are dropped.
    #[inline]
    pub(super) fn push(&mut self, id: u32, similarity: f32) {
        if self.k == 0 || similarity.is_nan() {
            return;
        }
        let candidate = SearchResult { id, similarity };
        if self.heap.len() < self.k {
            self.heap.push(Reverse(candidate));
        } else if let Some(Reverse(worst)) = self.heap.peek()
            && candidate > *worst
        {
            self.heap.pop();
            self.heap.push(Reverse(candidate));
        }
    }

    /// Results sorted best first.
    pub(super) fn into_sorted(self) -> Vec<SearchResult> {
        // Ascending order of `Reverse` is descending order of the score.
        self.heap
            .into_sorted_vec()
            .into_iter()
            .map(|r| r.0)
            .collect()
    }
}

/// Calculate Euclidean distance between two vectors
pub(super) fn euclidean_distance(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| (x - y).powi(2))
        .sum::<f32>()
        .sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_k_keeps_the_best_k_sorted_with_id_tiebreak() {
        let mut top = TopK::new(3);
        for (id, sim) in [
            (0, 0.1),
            (1, 0.9),
            (2, f32::NAN),
            (3, 0.5),
            (4, 0.9),
            (5, -1.0),
        ] {
            top.push(id, sim);
        }
        let got: Vec<_> = top.into_sorted().iter().map(|r| r.id).collect();
        assert_eq!(got, vec![1, 4, 3]);
        assert!(TopK::new(0).into_sorted().is_empty());
    }
}
