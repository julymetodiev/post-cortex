// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

/// Items over three waves, each spending `cost` steps; `fails` at those.
fn run(
    n: usize,
    steps: u64,
    cost: u64,
    fails: &[usize],
    stop_at: Option<usize>,
) -> (Result<(), Failure>, Vec<usize>, u64) {
    let mut meter = Meter::new(steps);
    let mut taken = Vec::new();
    let result = each(
        n,
        &mut meter,
        |i, meter| {
            meter.spend(cost);
            if fails.contains(&i) {
                Err(failure("expression", &format!("item {i}")))
            } else {
                Ok(i)
            }
        },
        |i, got| {
            assert_eq!(i, got);
            taken.push(got);
            Ok(stop_at != Some(i))
        },
    );
    (result, taken, meter.left())
}

#[test]
fn every_item_is_taken_in_order() {
    let (result, taken, left) = run(3_000, 1_000_000, 2, &[], None);
    assert!(result.is_ok());
    assert_eq!(taken, (0..3_000).collect::<Vec<_>>());
    assert_eq!(left, 1_000_000 - 6_000);
}

#[test]
fn the_first_failure_in_order_ends_it() {
    let (result, taken, _) = run(3_000, 1_000_000, 1, &[2_500, 1_500], None);
    let Err(failed) = result else {
        panic!("it did not fail");
    };
    assert_eq!(failed.message, "item 1500");
    assert_eq!(taken.len(), 1_500);
}

#[test]
fn it_stops_when_take_has_what_it_needs() {
    let (result, taken, left) = run(3_000, 1_000_000, 1, &[2_000], Some(1_100));
    assert!(result.is_ok());
    assert_eq!(taken.len(), 1_101);
    // The steps of the items after it are not spent.
    assert_eq!(left, 1_000_000 - 1_101);
}

#[test]
fn the_steps_run_out_where_one_item_after_another_would() {
    // 10 steps each, 15_005 in all: item 1500 spends past them, before
    // item 2000 fails.
    let (result, taken, left) = run(3_000, 15_005, 10, &[2_000], None);
    let Err(failed) = result else {
        panic!("it did not fail");
    };
    assert_eq!(failed.kind, kinds::BUDGET);
    assert_eq!(taken.len(), 1_500);
    assert_eq!(left, 0);
}
