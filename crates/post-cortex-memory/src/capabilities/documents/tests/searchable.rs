// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A kind that is not searchable (a module's log): its documents are kept
//! and read, never embedded, so search does not find them.

use std::time::Duration;

use super::*;
use crate::capabilities::SearchScope;
use crate::content_vectorizer::{ContentKind, SearchOptions};

#[tokio::test]
async fn a_kind_that_is_not_searchable_is_kept_but_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let memory = crate::memory_system::backends_tests::rocksdb_memory_in(
        &dir,
        crate::SystemConfig::default(),
    )
    .await
    .unwrap();
    let kinds = Fixed(vec![KindDeclaration {
        kind: task(),
        type_name: "pcx.v1.SessionInfo".to_string(),
        title_field: "name".to_string(),
        searchable: false,
        descriptors: post_cortex_proto::PCX_DESCRIPTOR_SET.to_vec(),
    }]);
    let board = Writer {
        module: Some("board"),
        kinds: &kinds,
        reach: None,
    };
    let ws = workspace(&memory).await;
    let mut req = put(ws, None, json!({"name": "Paint the lighthouse"}), 0);
    req.text = "The lighthouse keeper repaints the tower before winter".to_string();
    let written = memory
        .documents
        .put(req, board)
        .await
        .unwrap()
        .document
        .unwrap();
    assert!(memory.records.wait_idle(Duration::from_secs(120)).await);

    let read = memory.documents.get(ws, written.id, None).await.unwrap();
    assert_eq!(read.title, "Paint the lighthouse");
    let hits = memory
        .search
        .search(
            SearchScope::Workspace(ws),
            "lighthouse keeper repaints",
            SearchOptions::default(),
        )
        .await
        .unwrap();
    assert!(
        hits.iter().all(|h| h.content_type != ContentKind::Document),
        "{hits:?}"
    );
}
