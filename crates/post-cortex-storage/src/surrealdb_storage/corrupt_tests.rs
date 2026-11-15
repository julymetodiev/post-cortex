// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Unreadable rows are `Corrupt`, never defaults: a bad timestamp is not
//! the current time, an unknown role is not `Primary`, an unknown status
//! is not stale. And the `PersistedState` cache never drops a slot a save
//! may hold.

use post_cortex_ports::storage::{FreshnessStore, SessionStore, WorkspaceStore};

use uuid::Uuid;

use super::SurrealDBStorage;
use post_cortex_core::core::freshness::{SourceReference, SourceScope};
use post_cortex_core::session::active_session::ActiveSession;
use post_cortex_core::workspace::SessionRole;

async fn storage() -> SurrealDBStorage {
    SurrealDBStorage::new("mem://", None, None, None, None)
        .await
        .unwrap()
}

async fn run(storage: &SurrealDBStorage, query: &str, id: String) {
    storage
        .db()
        .query(query)
        .bind(("id", id))
        .await
        .unwrap()
        .check()
        .unwrap();
}

fn assert_corrupt<T: std::fmt::Debug>(result: post_cortex_ports::error::Result<T>) {
    let err = result.expect_err("an error");
    assert!(err.is_corrupt(), "{err:#}");
}

#[tokio::test]
async fn a_bad_timestamp_is_corrupt() {
    let storage = storage().await;
    let id = Uuid::new_v4();
    storage
        .save_session(
            &ActiveSession::new(id, None, None, chrono::Utc::now()),
            post_cortex_ports::storage::SessionChanges::default(),
        )
        .await
        .unwrap();
    run(
        &storage,
        "UPDATE type::record('session', $id) SET last_updated = 'yesterday';",
        id.to_string(),
    )
    .await;
    assert_corrupt(storage.load_session(id).await);
    assert_corrupt(storage.list_session_summaries().await);
}

#[tokio::test]
async fn an_unknown_role_is_corrupt() {
    let storage = storage().await;
    let ws = Uuid::new_v4();
    storage
        .save_workspace_metadata(ws, "ws", &ws.to_string(), "", &[])
        .await
        .unwrap();
    storage
        .add_session_to_workspace(ws, Uuid::new_v4(), SessionRole::Related)
        .await
        .unwrap();
    run(
        &storage,
        "UPDATE workspace_session SET role = 'Owner' WHERE workspace_id = $id;",
        ws.to_string(),
    )
    .await;
    assert_corrupt(storage.list_workspaces().await);
}

#[tokio::test]
async fn an_unknown_source_status_is_corrupt() {
    let storage = storage().await;
    let reference = SourceReference {
        entry_id: "e1".into(),
        file_path: "a.rs".into(),
        content_hash: b"h".to_vec(),
        captured_at_unix: 0,
        scope: SourceScope::WholeFile,
    };
    storage
        .register_source(Uuid::new_v4(), reference)
        .await
        .unwrap();
    run(
        &storage,
        "UPDATE source_reference SET status = 7 WHERE entry_id = $id;",
        "e1".into(),
    )
    .await;
    assert_corrupt(storage.check_freshness("e1", b"h").await);
    assert_corrupt(
        storage
            .check_freshness_batch(vec![("e1".into(), b"h".to_vec(), None, None)])
            .await,
    );
}
