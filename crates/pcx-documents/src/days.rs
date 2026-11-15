// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The days of a module's log of documents (mail's sends, http's calls):
//! the day an entry is of, which a daily limit counts, and whether an entry
//! is past the days the log keeps.

use chrono::{DateTime, Days, NaiveDate, Utc};

/// The day of `at` in UTC, `YYYY-MM-DD`.
#[must_use]
pub fn day(at: DateTime<Utc>) -> String {
    at.format("%Y-%m-%d").to_string()
}

/// Whether an entry of `day` is past the `keep` days a log keeps on
/// `today`.
///
/// # Errors
/// `day` is not `YYYY-MM-DD`: what it has.
pub fn expired(day: &str, today: DateTime<Utc>, keep: u64) -> Result<bool, String> {
    let day = NaiveDate::parse_from_str(day, "%Y-%m-%d")
        .map_err(|_| format!("has {day:?}, not YYYY-MM-DD"))?;
    let kept_from = today.date_naive().checked_sub_days(Days::new(keep));
    Ok(kept_from.is_some_and(|from| day < from))
}

#[cfg(test)]
mod tests;
