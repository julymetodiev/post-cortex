// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Migration from data version 5 (workspaces before remote reranking) to
//! version 6.

use super::*;
use crate::rocksdb_storage::legacy::{V5WorkspaceRow, WORKSPACE_V5};
use crate::rocksdb_storage::rows::encode_marked;

/// A workspace row of data version 5 keeps every field and gets the remote
/// reranker forbidden.
#[tokio::test]
async fn workspaces_of_version_5_forbid_the_remote_reranker() {
    let id = uid(9);
    let dir = tempdir().unwrap();
    {
        let storage = open(dir.path()).await;
        let row = V5WorkspaceRow {
            id,
            name: "Checkout".into(),
            slug: "checkout".into(),
            description: "d".into(),
            created_at: 10,
            projects: Vec::new(),
            archived_at: Some(20),
        };
        let value = encode_marked(WORKSPACE_V5, &row).unwrap();
        storage.db.put(format!("workspace:{id}"), value).unwrap();
        storage.db.put(DATA_VERSION_KEY, b"5").unwrap();
    }
    let storage = open(dir.path()).await;
    assert_eq!(data_version(&storage.db).unwrap(), DATA_VERSION);
    let ws = storage.list_workspaces().await.unwrap().remove(0);
    assert_eq!(
        (ws.id, ws.slug.as_str(), ws.created_at, ws.archived_at),
        (id, "checkout", 10, Some(20))
    );
    assert!(!ws.remote_rerank);
}
