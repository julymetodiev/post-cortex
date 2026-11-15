// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Migration from data version 3 (documents listed by `doc_kind` rows) to
//! the order and keys of version 4.

use super::*;
use crate::rocksdb_storage::rows::{DOCUMENT_V1, json_marked};
use post_cortex_core::core::document::{Document, DocumentData, DocumentKind};
use post_cortex_ports::storage::{DocumentStore, LatestQuery};

/// A version 3 document gets its place in its kind's order, unkeyed, and
/// its `doc_kind` row goes.
#[tokio::test]
async fn version_3_documents_are_ordered_and_unkeyed() {
    let (ws, id) = (Uuid::new_v4(), Uuid::new_v4());
    let kind: DocumentKind = "board/task".parse().unwrap();
    let document = Document {
        workspace_id: ws,
        id,
        kind: kind.clone(),
        version: 1,
        data: DocumentData::Message {
            type_name: "pcx.board.v1.Task".into(),
            bytes: vec![10, 1, b'a'],
        },
        title: "a".into(),
        text: String::new(),
        updated_at: post_cortex_ports::conformance::at(5),
        author: None,
    };
    let dir = tempdir().unwrap();
    {
        let storage = open(dir.path()).await;
        let db = &storage.db;
        db.put(
            format!("doc:{ws}:{id}:{:020}", 1),
            json_marked(DOCUMENT_V1, &document).unwrap(),
        )
        .unwrap();
        db.put(format!("doc_head:{ws}:{id}"), 1u64.to_be_bytes())
            .unwrap();
        db.put(format!("doc_kind:{ws}:{kind}:{id}"), []).unwrap();
        db.put(DATA_VERSION_KEY, b"3").unwrap();
    }
    let storage = open(dir.path()).await;
    assert_eq!(data_version(&storage.db).unwrap(), DATA_VERSION);
    let unkeyed = storage.unkeyed_documents(ws).await.unwrap();
    assert_eq!(unkeyed, vec![document.clone()]);
    let page = storage
        .latest_page(LatestQuery {
            workspace_id: ws,
            kind: &kind,
            keys: &[],
            after: None,
            limit: 10,
        })
        .await
        .unwrap();
    assert_eq!((page.documents, page.total), (vec![document], 1));
    assert!(
        storage
            .db
            .prefix_iterator(b"doc_kind:")
            .filter_map(Result::ok)
            .all(|(key, _)| !key.starts_with(b"doc_kind:")),
        "the doc_kind rows are gone"
    );
}
