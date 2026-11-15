// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Documents across an export and an import: every version goes with its
//! workspace; an import adds what continues the target's history, keeps a
//! diverged one (reported) unless `overwrite` replaces it.

use crate::capabilities::transfer::tests::{export, import, of_adapter, preview_of, rocksdb};
use crate::capabilities::transfer::*;
use post_cortex_core::core::document::{Document, DocumentData, DocumentDraft};
use post_cortex_ports::conformance::{Adapter, at};
use post_cortex_ports::storage::StoredWorkspace;
use uuid::Uuid;

fn version(workspace_id: Uuid, id: Uuid, n: u64, title: &str) -> Document {
    DocumentDraft {
        workspace_id,
        id,
        kind: "board/task".parse().unwrap(),
        data: DocumentData::Message {
            type_name: "pcx.board.v1.Task".into(),
            bytes: title.as_bytes().to_vec(),
        },
        title: title.to_string(),
        text: String::new(),
        author: None,
        keys: Vec::new(),
    }
    .into_version(n, at(n as i64))
    .0
}

async fn workspace_with(adapter: &Adapter, ws: Uuid, versions: Vec<Document>) {
    adapter
        .workspaces
        .restore_workspace(&StoredWorkspace {
            id: ws,
            name: "board".into(),
            slug: ws.to_string(),
            description: String::new(),
            sessions: vec![],
            created_at: 1_790_000_000,
            projects: vec![],
            archived_at: None,
            remote_rerank: false,
        })
        .await
        .unwrap();
    if let Some(first) = versions.first() {
        let id = first.id;
        adapter
            .documents
            .restore_document_versions(ws, id, versions, true, None)
            .await
            .unwrap();
    }
}

async fn history(adapter: &Adapter, ws: Uuid, id: Uuid) -> Vec<String> {
    let mut versions = adapter
        .documents
        .document_history(ws, id, 100)
        .await
        .unwrap();
    versions.reverse();
    versions
        .into_iter()
        .map(|d| format!("{}:{}", d.version, d.title))
        .collect()
}

#[tokio::test]
async fn documents_go_with_their_workspace_and_merge_by_history() {
    let (ws, id) = (Uuid::new_v4(), Uuid::new_v4());
    let (source, _s) = rocksdb().await;
    workspace_with(
        &source,
        ws,
        vec![version(ws, id, 1, "one"), version(ws, id, 2, "two")],
    )
    .await;
    let archive = export(
        &of_adapter(&source, 16),
        &ExportScope::Workspace(ws),
        &ExportOptions::default(),
    )
    .await
    .unwrap();
    assert_eq!(archive.metadata.document_count, 2);
    assert_eq!(archive.documents.len(), 2);

    // A target without it gets the whole history.
    let (empty, _e) = rocksdb().await;
    let ports = of_adapter(&empty, 16);
    let result = import(&ports, archive.clone(), &ImportOptions::default())
        .await
        .unwrap();
    assert_eq!(
        (result.documents_imported, result.documents.clone()),
        (1, vec![(ws, id)])
    );
    assert_eq!(history(&empty, ws, id).await, ["1:one", "2:two"]);
    // The workspace is there now: an import that skips it still merges its
    // documents, as it merges its card. Again: nothing to do.
    let skip = ImportOptions {
        skip_existing: true,
        ..ImportOptions::default()
    };
    let again = import(&ports, archive.clone(), &skip).await.unwrap();
    assert_eq!((again.documents_imported, again.documents_skipped), (0, 1));

    // A target one version behind gets the later one.
    let (behind, _b) = rocksdb().await;
    workspace_with(&behind, ws, vec![version(ws, id, 1, "one")]).await;
    let ports = of_adapter(&behind, 16);
    import(&ports, archive.clone(), &skip).await.unwrap();
    assert_eq!(history(&behind, ws, id).await, ["1:one", "2:two"]);

    // A diverged target keeps its history and the conflict is reported,
    // unless the import overwrites.
    let (diverged, _d) = rocksdb().await;
    workspace_with(
        &diverged,
        ws,
        vec![version(ws, id, 1, "one"), version(ws, id, 2, "mine")],
    )
    .await;
    let ports = of_adapter(&diverged, 16);
    let preview = preview_of(&ports, &archive, &skip).await.unwrap();
    assert_eq!(preview.documents, 1);
    assert!(
        preview
            .conflicts
            .iter()
            .any(|c| c.kind == "document" && c.id == id)
    );
    let kept = import(&ports, archive.clone(), &skip).await.unwrap();
    assert_eq!((kept.documents_imported, kept.documents_skipped), (0, 1));
    assert_eq!(history(&diverged, ws, id).await, ["1:one", "2:mine"]);
    let overwrite = ImportOptions {
        overwrite: true,
        ..ImportOptions::default()
    };
    import(&ports, archive, &overwrite).await.unwrap();
    assert_eq!(history(&diverged, ws, id).await, ["1:one", "2:two"]);
}
