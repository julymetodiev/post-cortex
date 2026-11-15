// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A task's place in its column (ARCHITECTURE section 8, W24): a rank, a
//! string of base-62 digits read as a fraction, so a task put between two
//! others gets a rank between theirs and no other task is written. A rank
//! never ends in the lowest digit, so there is always room before it.

/// The digits, in the order their bytes sort.
const DIGITS: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// The digits of `rank`, by value; none when a byte is no digit (a rank
/// not written here).
fn digits(rank: &str) -> Option<Vec<usize>> {
    rank.bytes()
        .map(|byte| DIGITS.iter().position(|d| *d == byte))
        .collect()
}

/// A rank after `low` and before `high`: `low` empty is the start, `high`
/// none the end.
///
/// A `high` that does not sort after `low` (two tasks written with one
/// rank) or a rank not written here is not a bound: the answer is after
/// `low` (`low` itself extended, when `low` is not of these digits).
#[must_use]
pub fn between(low: &str, high: Option<&str>) -> String {
    let Some(low_digits) = digits(low) else {
        return format!("{low}{}", char::from(DIGITS[DIGITS.len() / 2]));
    };
    let high = high.filter(|h| low < *h).and_then(digits);
    let mut rank = Vec::new();
    midpoint(&low_digits, high.as_deref(), &mut rank);
    rank.into_iter().map(|d| char::from(DIGITS[d])).collect()
}

/// The digits between `low` and `high`, appended to `rank`.
fn midpoint(low: &[usize], high: Option<&[usize]>, rank: &mut Vec<usize>) {
    // A common prefix (a missing digit of low reads as the lowest) is kept.
    if let Some(high) = high {
        let shared = high
            .iter()
            .enumerate()
            .take_while(|(i, d)| low.get(*i).copied().unwrap_or(0) == **d)
            .count();
        if shared > 0 {
            rank.extend_from_slice(&high[..shared]);
            let rest = low.get(shared..).unwrap_or(&[]);
            midpoint(rest, Some(&high[shared..]), rank);
            return;
        }
    }
    let first_low = low.first().copied().unwrap_or(0);
    let first_high = high
        .and_then(|h| h.first())
        .copied()
        .unwrap_or(DIGITS.len());
    if first_high - first_low > 1 {
        rank.push(usize::midpoint(first_low, first_high));
        return;
    }
    // The first digits are neighbours.
    match high {
        // High's first digit alone sorts after low and before high.
        Some(high) if high.len() > 1 => rank.push(high[0]),
        _ => {
            rank.push(first_low);
            midpoint(low.get(1..).unwrap_or(&[]), None, rank);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rank_sorts_between_its_neighbours() {
        let first = between("", None);
        let after = between(&first, None);
        let before = between("", Some(&first));
        let middle = between(&first, Some(&after));
        assert!(before < first && first < middle && middle < after);
        for rank in [&first, &after, &before, &middle] {
            assert!(!rank.ends_with('0'), "{rank} leaves room before it");
        }
    }

    #[test]
    fn many_moves_keep_the_order() {
        // Always between the first two: the deepest case.
        let mut ranks = vec![between("", None)];
        ranks.push(between(&ranks[0], None));
        for _ in 0..200 {
            let rank = between(&ranks[0], Some(&ranks[1]));
            ranks.insert(1, rank);
        }
        // Always at the start.
        for _ in 0..200 {
            let rank = between("", Some(&ranks[0]));
            ranks.insert(0, rank);
        }
        assert!(ranks.windows(2).all(|w| w[0] < w[1]));
        assert!(ranks.iter().all(|r| r.len() < 64), "ranks stay short");
    }

    #[test]
    fn equal_neighbours_or_foreign_ranks_give_a_rank_after_the_lower() {
        assert!(between("V", Some("V")).as_str() > "V");
        assert!(between("V", Some("~")).as_str() > "V");
        assert!(between("é", None).as_str() > "é");
    }
}
