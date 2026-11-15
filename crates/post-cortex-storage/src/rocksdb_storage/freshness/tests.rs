// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;
use post_cortex_core::core::freshness::{BlockScope, FunctionScope, SourceScope};
use post_cortex_ports::storage::FreshnessStore;
use tempfile::tempdir;

fn reference(entry_id: &str, file: &str, scope: SourceScope) -> SourceReference {
    SourceReference {
        entry_id: entry_id.to_string(),
        file_path: file.to_string(),
        content_hash: b"file-hash".to_vec(),
        captured_at_unix: 0,
        scope,
    }
}

#[tokio::test]
async fn file_index_drives_lookup_and_invalidation() {
    let dir = tempdir().unwrap();
    let storage = RealRocksDBStorage::new(dir.path()).await.unwrap();
    let sid = uuid::Uuid::new_v4();
    let whole = SourceScope::WholeFile;

    for (entry, file) in [
        ("e1", "a.rs"),
        ("e2", "a.rs"),
        ("e3", "a.rs|b"),
        ("e4", "b.rs"),
    ] {
        storage
            .register_source(sid, reference(entry, file, whole.clone()))
            .await
            .unwrap();
    }
    // e4 moves to a.rs: its old index entry must not linger.
    storage
        .register_source(sid, reference("e4", "a.rs", whole))
        .await
        .unwrap();

    let mut ids: Vec<String> = storage
        .get_entries_by_source("a.rs")
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.entry_id)
        .collect();
    ids.sort();
    assert_eq!(ids, vec!["e1", "e2", "e4"]);
    assert!(
        storage
            .get_entries_by_source("b.rs")
            .await
            .unwrap()
            .is_empty()
    );

    let invalidated = storage
        .invalidate_source(InvalidationScope::Session(sid), "a.rs")
        .await
        .unwrap();
    assert_eq!(invalidated, 3);
    assert_eq!(
        storage
            .get_stale_entries_by_source("a.rs")
            .await
            .unwrap()
            .len(),
        3
    );
    assert!(
        storage
            .get_stale_entries_by_source("a.rs|b")
            .await
            .unwrap()
            .is_empty()
    );
}

/// The row keeps every scope as registered, line ranges included; the
/// gRPC schema is not involved.
#[tokio::test]
async fn rows_keep_every_scope() {
    let dir = tempdir().unwrap();
    let storage = RealRocksDBStorage::new(dir.path()).await.unwrap();
    let sid = uuid::Uuid::new_v4();
    let scopes = [
        SourceScope::WholeFile,
        SourceScope::Function(FunctionScope {
            name: "foo".into(),
            ast_hash: vec![1, 2],
            symbol_type: "fn".into(),
            imports: vec!["Bar".into()],
        }),
        SourceScope::Block(BlockScope {
            start_line: 3,
            end_line: 9,
            content_hash: vec![7],
        }),
    ];
    for (i, scope) in scopes.iter().enumerate() {
        storage
            .register_source(sid, reference(&format!("e{i}"), "x.rs", scope.clone()))
            .await
            .unwrap();
    }
    let mut stored = storage.get_entries_by_source("x.rs").await.unwrap();
    stored.sort_by(|a, b| a.entry_id.cmp(&b.entry_id));
    let got: Vec<SourceScope> = stored.into_iter().map(|r| r.scope).collect();
    assert_eq!(got, scopes);
}

/// An unreadable reference row is `Corrupt`, in single and batch checks
/// (it was reported `Unknown`).
#[tokio::test]
async fn an_unreadable_row_is_corrupt() {
    let dir = tempdir().unwrap();
    let storage = RealRocksDBStorage::new(dir.path()).await.unwrap();
    storage.db.put(b"source_ref:bad", b"\x00\x07junk").unwrap();
    let err = storage.check_freshness("bad", b"h").await.unwrap_err();
    assert!(err.is_corrupt(), "{err:#}");
    let err = storage
        .check_freshness_batch(vec![("bad".into(), b"h".to_vec(), None, None)])
        .await
        .unwrap_err();
    assert!(err.is_corrupt(), "{err:#}");
}
