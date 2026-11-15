// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;
use crate::memory_system::write_path_tests::text_record;

#[test]
fn pending_counts_each_record_once() {
    let mut session = ActiveSession::new(Uuid::new_v4(), None, None, chrono::Utc::now());
    for i in 0..3 {
        session.apply_record(text_record(&format!("record number {i}")));
    }
    let ids = VectorizedIds::default();
    assert_eq!(ids.pending_count(&session), 3);
    let first = session.records()[0].id;
    ids.mark(first);
    ids.mark(first);
    assert_eq!(ids.pending_count(&session), 2);
    assert_eq!(ids.len(), 1);
    ids.unmark(&first);
    assert_eq!(ids.pending_count(&session), 3);
    assert!(ids.is_empty());
}

#[test]
fn stored_ids_load_marked() {
    let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
    let ids = VectorizedIds::new([a, b]);
    let mut back = ids.ids();
    back.sort();
    let mut want = vec![a, b];
    want.sort();
    assert_eq!(back, want);
    assert!(ids.contains(&a) && !ids.contains(&Uuid::new_v4()));
}

/// A save takes the marks and unmarks made since the last one, the latest
/// of each id; stored ids are not unsaved.
#[test]
fn unsaved_changes_are_taken_once() {
    let (a, b, c) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let ids = VectorizedIds::new([c]);
    ids.mark(a);
    ids.mark(b);
    ids.mark(a);
    ids.unmark(&b);
    ids.unmark(&c);
    assert_eq!(ids.take_unsaved(), (vec![a], vec![b, c]));
    assert_eq!(ids.take_unsaved(), (vec![], vec![]));
    ids.mark(c);
    assert_eq!(ids.take_unsaved(), (vec![c], vec![]));
}
