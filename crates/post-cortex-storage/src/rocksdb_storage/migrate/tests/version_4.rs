// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Migration from data version 4 (workspaces without slugs) to version 5.

use super::*;
use crate::rocksdb_storage::legacy::{V4WorkspaceRow, WORKSPACE_V4};
use crate::rocksdb_storage::rows::encode_marked;

/// The workspace rows of data version 4 get their slugs, the oldest first:
/// a later one of the same name carries its id.
#[tokio::test]
async fn workspaces_of_version_4_get_their_slugs() {
    let (older, newer) = (uid(7), uid(8));
    let dir = tempdir().unwrap();
    {
        let storage = open(dir.path()).await;
        for (id, created_at) in [(newer, 20), (older, 10)] {
            let row = V4WorkspaceRow {
                id,
                name: "Checkout".into(),
                description: String::new(),
                created_at,
                projects: Vec::new(),
                archived_at: None,
            };
            let value = encode_marked(WORKSPACE_V4, &row).unwrap();
            storage.db.put(format!("workspace:{id}"), value).unwrap();
        }
        storage.db.put(DATA_VERSION_KEY, b"4").unwrap();
    }
    let storage = open(dir.path()).await;
    assert_eq!(data_version(&storage.db).unwrap(), DATA_VERSION);
    let slugs: std::collections::HashMap<Uuid, String> = storage
        .list_workspaces()
        .await
        .unwrap()
        .into_iter()
        .map(|w| (w.id, w.slug))
        .collect();
    let hex = newer.simple().to_string();
    assert_eq!(slugs[&older], "checkout");
    assert_eq!(slugs[&newer], format!("checkout-{}", &hex[..6]));
}
