// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;
use crate::rocksdb_storage::migrate::tests::fixture_row;

const S1: &str = "00050000-0000-4000-8000-000000000001";

fn exact<T: DeserializeOwned>(bytes: &[u8]) -> T {
    bincode_exact("test", "fixture", bytes).unwrap()
}

/// Layout 4 of an update row is the bincode `ContextUpdate` 0.5.0 wrote:
/// the fixture's rows decode as exactly that and encode back to the same
/// bytes. Fails when `ContextUpdate` changes its fields; then freeze the
/// layout in `legacy.rs`, bump the marker and add a migration step.
#[test]
fn update_layout_4_is_pinned() {
    for n in ["101", "102", "103"] {
        let row = fixture_row(&format!(
            "session:{S1}:update:00050000-0000-4000-8000-000000000{n}"
        ));
        let update: ContextUpdate = exact(&row);
        let marked = encode_update(&update).unwrap();
        assert_eq!(&marked[UPDATE_V4.len()..], row.as_slice(), "update {n}");
        assert_eq!(decode_update("k", &marked).unwrap().id, update.id);
    }
}

/// Checkpoint layout 4 is pinned the same way (the fixture's checkpoint is
/// already marked `pcx-ckpt-v4`).
#[test]
fn checkpoint_layout_4_is_pinned() {
    let row = fixture_row("checkpoint:00050000-0000-4000-8000-000000000003");
    let checkpoint = decode_checkpoint("k", &row).unwrap();
    assert_eq!(encode_checkpoint(&checkpoint).unwrap(), row);
}

/// A row read with the wrong marker, an unmarked row or one with bytes
/// after its body is `Corrupt`, never a default or a misread.
#[test]
fn rows_decode_only_their_own_layout() {
    let membership = MembershipRow::new(Uuid::new_v4(), Uuid::new_v4(), SessionRole::Dependency, 7);
    let row = encode_membership(&membership).unwrap();
    assert_eq!(decode_membership("k", &row).unwrap(), membership);
    assert_eq!(
        decode_membership("k", &row).unwrap().role().unwrap(),
        SessionRole::Dependency
    );

    let corrupt = |result: Result<()>| {
        let err = result.expect_err("an error");
        assert!(
            matches!(&err, crate::port_error::AdapterError::Port(p) if p.is_corrupt()),
            "{err:#}"
        );
    };
    corrupt(decode_workspace("k", &row).map(|_| ()));
    corrupt(decode_membership("k", &row[MEMBERSHIP_V1.len()..]).map(|_| ()));
    let mut longer = row.clone();
    longer.push(0);
    corrupt(decode_membership("k", &longer).map(|_| ()));
    let unknown_role = MembershipRow {
        role: "owner".into(),
        ..membership
    };
    corrupt(unknown_role.role().map(|_| ()));
    corrupt(crate::rocksdb_storage::v2_layout::decode_session("k", b"{}").map(|_| ()));
    corrupt(decode_head("k", b"{}").map(|_| ()));
    corrupt(decode_entity("k", b"x").map(|_| ()));
    corrupt(decode_edge("k", b"x").map(|_| ()));
}
