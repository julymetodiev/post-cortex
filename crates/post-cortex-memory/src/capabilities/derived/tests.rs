// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

/// A burst of full-queue fallbacks warns once, then once a minute with
/// the count.
#[test]
fn backpressure_is_summarised() {
    let log = BackpressureLog::default();
    assert_eq!(log.note("embedding").map(|(n, _)| n), Some(1));
    for _ in 0..4_999 {
        assert!(log.note("embedding").is_none());
    }
    assert_eq!(
        log.note("graph").map(|(n, _)| n),
        Some(1),
        "queues are separate"
    );
    log.queues.lock()[0].1 -= BACKPRESSURE_WARN_EVERY;
    assert_eq!(log.note("embedding").map(|(n, _)| n), Some(5_000));
    assert!(log.note("embedding").is_none());
}
