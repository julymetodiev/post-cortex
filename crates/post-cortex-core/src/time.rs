// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The one reading of a time a caller writes: RFC 3339
//! (`2026-10-22T09:30:00Z`), a date and time in UTC (`2026-10-22 09:30:00`)
//! or a date (`2026-10-22`, midnight UTC).

use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};

/// A UTC range, both ends inclusive.
pub type TimeRange = (DateTime<Utc>, DateTime<Utc>);

/// The time `value` names, or `None` when it is in none of the accepted
/// forms.
pub fn parse_time(value: &str) -> Option<DateTime<Utc>> {
    let value = value.trim();
    DateTime::parse_from_rfc3339(value)
        .map(|t| t.with_timezone(&Utc))
        .ok()
        .or_else(|| {
            NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S")
                .ok()
                .map(|t| t.and_utc())
        })
        .or_else(|| {
            NaiveDate::parse_from_str(value, "%Y-%m-%d")
                .ok()
                .and_then(|d| d.and_hms_opt(0, 0, 0))
                .map(|t| t.and_utc())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_accepted_form_reads_the_same_instant() {
        let midnight = parse_time("2026-10-22T00:00:00Z").unwrap();
        assert_eq!(parse_time("2026-10-22"), Some(midnight));
        assert_eq!(parse_time("2026-10-22 00:00:00"), Some(midnight));
        assert_eq!(parse_time(" 2026-10-22T02:00:00+02:00 "), Some(midnight));
        assert_eq!(parse_time("yesterday"), None);
    }
}
