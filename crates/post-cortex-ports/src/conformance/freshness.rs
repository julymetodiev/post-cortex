// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! Freshness: one state machine on every backend. A registered reference is
//! fresh while its hashes match, an invalidated one (by file or by cascade)
//! is stale and listed as stale until it is registered again, and an
//! invalidation scoped to a session leaves other sessions' references alone.

use crate::storage::Stores;
use post_cortex_core::core::freshness::{
    BlockScope, FreshnessStatus, FunctionScope, SourceReference, SourceScope, SymbolId,
};
use uuid::Uuid;

use crate::storage::InvalidationScope;

fn reference(entry_id: &str, file: &str, func: &str) -> SourceReference {
    SourceReference {
        entry_id: entry_id.to_string(),
        file_path: file.to_string(),
        content_hash: b"file-hash".to_vec(),
        captured_at_unix: 0,
        scope: SourceScope::Function(FunctionScope {
            name: func.to_string(),
            ast_hash: format!("ast-{func}").into_bytes(),
            symbol_type: "fn".to_string(),
            imports: vec![],
        }),
    }
}

async fn status(stores: &Stores, entry_id: &str) -> FreshnessStatus {
    stores
        .freshness
        .check_freshness(entry_id, b"file-hash")
        .await
        .unwrap()
        .status
}

async fn stale_ids(stores: &Stores, file: &str) -> Vec<String> {
    let mut ids: Vec<String> = stores
        .freshness
        .get_stale_entries_by_source(file)
        .await
        .unwrap()
        .into_iter()
        .map(|s| s.entry_id)
        .collect();
    ids.sort();
    ids
}

/// registered -> stale (invalidate) -> registered again; what stays stale
/// after re-registration is an orphan.
pub async fn invalidated_references_are_stale_until_registered_again(stores: &Stores) {
    let sid = Uuid::new_v4();
    stores
        .freshness
        .register_source(sid, reference("e1", "a.rs", "foo"))
        .await
        .unwrap();
    stores
        .freshness
        .register_source(sid, reference("e2", "a.rs", "bar"))
        .await
        .unwrap();
    assert_eq!(status(stores, "e1").await, FreshnessStatus::Fresh,);

    let invalidated = stores
        .freshness
        .invalidate_source(InvalidationScope::Session(sid), "a.rs")
        .await
        .unwrap();
    assert_eq!(invalidated, 2);
    assert_eq!(status(stores, "e1").await, FreshnessStatus::Stale,);
    assert_eq!(stale_ids(stores, "a.rs").await, ["e1", "e2"]);
    assert_eq!(
        stores
            .freshness
            .get_entries_by_source("a.rs")
            .await
            .unwrap()
            .len(),
        2,
        "an invalidated reference is kept"
    );

    // e1 is still in the file: registered again, fresh; e2 is the orphan.
    stores
        .freshness
        .register_source(sid, reference("e1", "a.rs", "foo"))
        .await
        .unwrap();
    assert_eq!(status(stores, "e1").await, FreshnessStatus::Fresh,);
    assert_eq!(stale_ids(stores, "a.rs").await, ["e2"]);
}

/// Cascade invalidation marks stale too (never forgets the reference).
pub async fn cascade_marks_stale(stores: &Stores) {
    let sid = Uuid::new_v4();
    let symbol = |file: &str, name: &str| SymbolId {
        file_path: file.to_string(),
        symbol_name: name.to_string(),
        symbol_type: "fn".to_string(),
    };
    stores
        .freshness
        .register_source(sid, reference("core", "lib.rs", "core"))
        .await
        .unwrap();
    stores
        .freshness
        .register_source(sid, reference("user", "app.rs", "user"))
        .await
        .unwrap();
    stores
        .freshness
        .register_symbol_dependencies(symbol("app.rs", "user"), vec![symbol("lib.rs", "core")])
        .await
        .unwrap();

    let report = stores
        .freshness
        .cascade_invalidate(symbol("lib.rs", "core"), vec![], 3)
        .await
        .unwrap();
    assert_eq!(
        (report.direct_invalidations, report.cascade_invalidations),
        (1, 1),
    );
    assert_eq!(status(stores, "core").await, FreshnessStatus::Stale,);
    assert_eq!(status(stores, "user").await, FreshnessStatus::Stale,);
    assert_eq!(stale_ids(stores, "app.rs").await, ["user"]);
}

/// A session's invalidation leaves another session's references fresh; the
/// explicit all-sessions invalidation reaches both.
pub async fn invalidation_is_scoped_to_the_session(stores: &Stores) {
    let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
    stores
        .freshness
        .register_source(a, reference("a1", "shared.rs", "f"))
        .await
        .unwrap();
    stores
        .freshness
        .register_source(b, reference("b1", "shared.rs", "g"))
        .await
        .unwrap();

    let invalidated = stores
        .freshness
        .invalidate_source(InvalidationScope::Session(a), "shared.rs")
        .await
        .unwrap();
    assert_eq!(invalidated, 1);
    assert_eq!(status(stores, "a1").await, FreshnessStatus::Stale,);
    assert_eq!(
        status(stores, "b1").await,
        FreshnessStatus::Fresh,
        "another session's reference"
    );

    let invalidated = stores
        .freshness
        .invalidate_source(InvalidationScope::AllSessions, "shared.rs")
        .await
        .unwrap();
    assert_eq!(invalidated, 2);
    assert_eq!(status(stores, "b1").await, FreshnessStatus::Stale,);
}

/// One batch answers as the single checks would, the AST hash winning over
/// the file hash; an entry without a reference is `Unknown`.
pub async fn batch_check_matches_single_checks(stores: &Stores) {
    let sid = Uuid::new_v4();
    stores
        .freshness
        .register_source(sid, reference("fn1", "a.rs", "foo"))
        .await
        .unwrap();
    let file = SourceReference {
        scope: SourceScope::WholeFile,
        ..reference("file1", "a.rs", "-")
    };
    stores
        .freshness
        .register_source(sid, file.clone())
        .await
        .unwrap();

    let entries = vec![
        // AST hash matches although the file changed: fresh.
        (
            "fn1".to_string(),
            b"changed".to_vec(),
            Some(b"ast-foo".to_vec()),
            None,
        ),
        // AST hash differs: stale.
        (
            "fn1".to_string(),
            b"file-hash".to_vec(),
            Some(b"other".to_vec()),
            None,
        ),
        // File-level reference, hash unchanged: fresh.
        ("file1".to_string(), b"file-hash".to_vec(), None, None),
        ("missing".to_string(), b"x".to_vec(), None, None),
    ];
    let mut single = Vec::new();
    for (id, file_hash, ast, name) in &entries {
        single.push(
            stores
                .freshness
                .check_freshness_semantic(id, file_hash, ast.as_deref(), name.as_deref())
                .await
                .unwrap(),
        );
    }
    let batch = stores
        .freshness
        .check_freshness_batch(entries)
        .await
        .unwrap();
    assert_eq!(batch, single);
    let statuses: Vec<FreshnessStatus> = batch.iter().map(|e| e.status).collect();
    assert_eq!(
        statuses,
        [
            FreshnessStatus::Fresh,
            FreshnessStatus::Stale,
            FreshnessStatus::Fresh,
            FreshnessStatus::Unknown,
        ]
    );
    let stored = stores
        .freshness
        .get_entries_by_source("a.rs")
        .await
        .unwrap();
    assert!(stored.contains(&file), "{stored:?}");
}

/// Every scope reads back as it was registered: whole file, function and
/// block (line range). SurrealDB stored only function scopes before, and a
/// block read back as the whole file.
pub async fn every_scope_round_trips(stores: &Stores) {
    let sid = Uuid::new_v4();
    let block = SourceReference {
        entry_id: "blk".into(),
        file_path: "b.rs".into(),
        content_hash: b"file-hash".to_vec(),
        captured_at_unix: 7,
        scope: SourceScope::Block(BlockScope {
            start_line: 10,
            end_line: 24,
            content_hash: b"block-hash".to_vec(),
        }),
    };
    let whole = SourceReference {
        entry_id: "whole".into(),
        scope: SourceScope::WholeFile,
        ..block.clone()
    };
    let func = SourceReference {
        file_path: "b.rs".into(),
        ..reference("fn", "b.rs", "foo")
    };
    for r in [&block, &whole, &func] {
        stores
            .freshness
            .register_source(sid, r.clone())
            .await
            .unwrap();
    }
    let mut stored = stores
        .freshness
        .get_entries_by_source("b.rs")
        .await
        .unwrap();
    stored.sort_by(|a, b| a.entry_id.cmp(&b.entry_id));
    assert_eq!(stored, [block, func, whole]);
}
