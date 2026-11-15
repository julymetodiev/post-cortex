// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A data block's items evaluated in waves over the cores
//! (docs/modules/workflow.md, data blocks, speed). The items of a wave are
//! evaluated at once, each on a copy of the block's meter; their results
//! and their steps are then taken in order, as one item after another
//! would be: the block stops at the first failure, when its steps run out,
//! or when it has what it needs (a filter's limit). What a block gives does
//! not depend on the cores; at most one wave's work is done past where it
//! stops.

use pcx_expr::Meter;
use post_cortex_proto::workflow::Failure;
use rayon::prelude::*;

use crate::domain::retry::{failure, kinds};

/// Items a wave evaluates at once.
const WAVE: usize = 1_024;

/// Items one core takes at least: a short list stays on one.
const LEAST: usize = 64;

/// Items `0..n`, each evaluated by `work` on its own copy of `meter`, and
/// taken in order by `take` until it says to stop (false); each item's
/// steps spent on `meter` before it is taken.
///
/// # Errors
/// The first item's failure, in order; `budget` when the items before it
/// and its own spent more steps than `meter` has; `take`'s.
pub fn each<T: Send>(
    n: usize,
    meter: &mut Meter,
    work: impl Fn(usize, &mut Meter) -> Result<T, Failure> + Sync,
    mut take: impl FnMut(usize, T) -> Result<bool, Failure>,
) -> Result<(), Failure> {
    let mut start = 0;
    while start < n {
        let end = n.min(start + WAVE);
        let at = *meter;
        let done: Vec<(Result<T, Failure>, u64)> = (start..end)
            .into_par_iter()
            .with_min_len(LEAST)
            .map(|i| {
                let mut own = at;
                let result = work(i, &mut own);
                (result, at.left() - own.left())
            })
            .collect();
        for (i, (result, steps)) in (start..end).zip(done) {
            if !meter.charge(steps) {
                return Err(failure(
                    kinds::BUDGET,
                    &format!("the block spent its {} steps", meter.all()),
                ));
            }
            if !take(i, result?)? {
                return Ok(());
            }
        }
        start = end;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
