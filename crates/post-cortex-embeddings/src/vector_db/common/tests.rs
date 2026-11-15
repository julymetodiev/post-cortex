// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

fn result(slot: u32, score: f32, key: u64) -> SearchResult {
    SearchResult {
        slot,
        score,
        key: (key, 0),
    }
}

#[test]
fn top_k_keeps_the_best_k_sorted_with_key_then_slot_tiebreak() {
    let mut top = TopK::new(3);
    for (slot, score, key) in [
        (0, 0.1, 0),
        (1, 0.9, 7),
        (2, f32::NAN, 0),
        (3, 0.5, 0),
        (4, 0.9, 2),
        (5, -1.0, 0),
    ] {
        top.push(result(slot, score, key));
    }
    let got: Vec<_> = top.into_sorted().iter().map(|r| r.slot).collect();
    assert_eq!(got, vec![4, 1, 3]);
    assert!(TopK::new(0).into_sorted().is_empty());
}

#[test]
fn merged_collectors_keep_the_best_of_both() {
    let mut a = TopK::new(2);
    let mut b = TopK::new(2);
    a.push(result(0, 0.2, 0));
    a.push(result(1, 0.8, 0));
    b.push(result(2, 0.9, 0));
    b.push(result(3, 0.1, 0));
    let got: Vec<_> = a.merge(b).into_sorted().iter().map(|r| r.slot).collect();
    assert_eq!(got, vec![2, 1]);
}
