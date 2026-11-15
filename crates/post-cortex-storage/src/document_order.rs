// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The order both stores keep the latest versions of documents in
//! (docs/memory.md, document keys and pages): the time of the latest
//! version inverted, so an ascending walk gives the most recent change
//! first, then the id.

use chrono::{DateTime, Utc};
use post_cortex_core::core::document::DocumentCursor;
use post_cortex_ports::error::corrupt;
use uuid::Uuid;

use crate::port_error::Result;

/// `at` inverted: `i64::MAX` less its nanoseconds since 1970, so a later
/// time is a smaller number. A time before 1970 or past 2262 has none.
pub fn inverted(at: DateTime<Utc>, id: &str) -> Result<i64> {
    match at.timestamp_nanos_opt() {
        Some(nanos) if nanos >= 0 => Ok(i64::MAX - nanos),
        _ => Err(corrupt("document time", id, format!("{at} has no order")).into()),
    }
}

/// The page of `matched` (every match, in order: inverted time, id, and
/// what the store reads it by) after `after`, at most `limit`.
pub struct Window<T> {
    /// The page.
    pub page: Vec<T>,
    /// Every match.
    pub total: usize,
    /// Whether more follow the page.
    pub more: bool,
}

/// The [`Window`] of `matched` after `after`.
pub fn window<T>(
    matched: Vec<(i64, Uuid, T)>,
    after: Option<&DocumentCursor>,
    limit: usize,
) -> Result<Window<T>> {
    let total = matched.len();
    let start = match after {
        Some(c) => Some((inverted(c.at, &c.id.to_string())?, c.id)),
        None => None,
    };
    let mut past = matched
        .into_iter()
        .filter(|(inv, id, _)| start.is_none_or(|s| (*inv, *id) > s))
        .map(|(_, _, read)| read);
    let page: Vec<T> = past.by_ref().take(limit).collect();
    let more = past.next().is_some();
    Ok(Window { page, total, more })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn a_window_starts_after_its_cursor_and_says_if_more_follow() {
        let at = |s| Utc.timestamp_opt(s, 0).single().unwrap();
        let ids: Vec<Uuid> = (1..=4).map(Uuid::from_u128).collect();
        let matched: Vec<_> = (0..4)
            .map(|i| (inverted(at(10 - i), "x").unwrap(), ids[i as usize], i))
            .collect();
        let first = window(matched.clone(), None, 2).unwrap();
        assert_eq!((first.page, first.total, first.more), (vec![0, 1], 4, true));
        let cursor = DocumentCursor {
            at: at(9),
            id: ids[1],
        };
        let rest = window(matched, Some(&cursor), 2).unwrap();
        assert_eq!((rest.page, rest.total, rest.more), (vec![2, 3], 4, false));
    }

    #[test]
    fn a_later_time_orders_first_and_1970_is_the_last() {
        let early = Utc.timestamp_opt(1, 0).single().unwrap();
        let late = Utc.timestamp_opt(2, 5).single().unwrap();
        assert!(inverted(late, "x").unwrap() < inverted(early, "x").unwrap());
        assert_eq!(inverted(DateTime::UNIX_EPOCH, "x").unwrap(), i64::MAX);
        let before = Utc.timestamp_opt(-1, 0).single().unwrap();
        assert!(inverted(before, "x").is_err());
    }
}
