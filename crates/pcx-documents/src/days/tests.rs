// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

#[test]
fn a_log_keeps_its_days() {
    let today = DateTime::parse_from_rfc3339("2026-10-31T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    assert_eq!(day(today), "2026-10-31");
    assert_eq!(expired("2026-08-02", today, 90), Ok(false));
    assert_eq!(expired("2026-08-01", today, 90), Ok(true));
    assert_eq!(expired("2026-10-01", today, 30), Ok(false));
    assert_eq!(expired("2026-09-30", today, 30), Ok(true));
    assert!(expired("31.10.2026", today, 90).is_err());
}
