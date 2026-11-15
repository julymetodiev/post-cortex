// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Export format 1.10.0 is pinned byte for byte: an archive of fixed
//! content (a session with records, graph and embedded record ids, its
//! workspace allowing the remote reranker, a checkpoint, a vector, a captured message and an agent's
//! tool call with its parent and blocks, a card and a document with two
//! versions, its data a message) exported from RocksDB, part by part,
//! writes exactly `tests/fixtures/archive-1.10.0.json`. The archives of
//! 1.9.0 (before persons' user cards), 1.8.0 (before remote reranking), 1.7.0 (before workspace slugs), 1.6.0 (documents' data as JSON), 1.5.0 (written before messages had
//! parents and blocks) and 1.4.1 (the whole-archive writer, before
//! documents) still import. Regenerate the 1.10.0 file with
//! `PCX_WRITE_GOLDEN=1`.

use crate::capabilities::transfer::*;
use post_cortex_storage::RealRocksDBStorage;
use std::collections::HashMap;
use std::sync::Arc;

use post_cortex_core::core::card::{Card, CardScope};
use post_cortex_core::core::document::{DocumentData, DocumentDraft};
use post_cortex_core::core::message::{MessageBlock, MessageRole, NewMessage};
use post_cortex_core::session::active_session::ActiveSession;
use post_cortex_core::workspace::SessionRole;
use post_cortex_ports::conformance::checkpoints::checkpoint;
use post_cortex_ports::conformance::{at, record};
use post_cortex_ports::storage::VectorStore;
use post_cortex_ports::storage::{StoredWorkspace, Stores};
use post_cortex_ports::vectors::VectorMetadata;
use uuid::Uuid;

const GOLDEN: &str = include_str!("../../../../tests/fixtures/archive-1.10.0.json");
const GOLDEN_1_9_0: &str = include_str!("../../../../tests/fixtures/archive-1.9.0.json");
const GOLDEN_1_8_0: &str = include_str!("../../../../tests/fixtures/archive-1.8.0.json");
const GOLDEN_1_7_0: &str = include_str!("../../../../tests/fixtures/archive-1.7.0.json");
const GOLDEN_1_6_0: &str = include_str!("../../../../tests/fixtures/archive-1.6.0.json");
const GOLDEN_1_5_0: &str = include_str!("../../../../tests/fixtures/archive-1.5.0.json");
const GOLDEN_1_4_1: &str = include_str!("../../../../tests/fixtures/archive-1.4.1.json");

fn id(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

/// Every section of an archive, with fixed ids and times, exported
/// (indented JSON).
async fn golden_archive() -> Vec<u8> {
    let dir = tempfile::tempdir().unwrap();
    let storage = Arc::new(RealRocksDBStorage::new(dir.path()).await.unwrap());
    let stores = Stores::of(Arc::clone(&storage));
    let (sid, wid) = (id(1), id(2));

    let mut session = ActiveSession::new(sid, Some("golden".into()), Some("pinned".into()), at(0));
    for (n, (title, entities)) in [("storage", &["pcx", "RocksDB"][..]), ("search", &["pcx"])]
        .into_iter()
        .enumerate()
    {
        let mut r = record(n as i64 + 1, title, entities);
        r.id = id(10 + n as u128);
        session.apply_record(r);
    }
    let first = session.records()[0].id;
    stores
        .sessions
        .save_session(
            &session,
            post_cortex_ports::storage::SessionChanges::everything(&session, &[first]),
        )
        .await
        .unwrap();

    stores
        .workspaces
        .restore_workspace(&StoredWorkspace {
            id: wid,
            name: "golden ws".into(),
            slug: "golden-ws".into(),
            description: "pinned".into(),
            sessions: vec![(sid, SessionRole::Primary)],
            created_at: 1_790_000_000,
            projects: vec![],
            archived_at: None,
            remote_rerank: true,
        })
        .await
        .unwrap();

    let mut cp = checkpoint(sid);
    cp.id = id(3);
    cp.recent_updates[0].id = id(30);
    stores.checkpoints.save_checkpoint(&cp).await.unwrap();

    storage
        .upsert_vectors(vec![(
            vec![0.25, -0.5, 1.0],
            VectorMetadata {
                id: first.to_string(),
                text: "about storage".into(),
                source: sid.to_string(),
                content_type: "UpdateContent".into(),
                timestamp: at(1),
                metadata: HashMap::from([("author".to_string(), "claude-code".to_string())]),
            },
        )])
        .await
        .unwrap();

    stores
        .messages
        .append_messages(
            sid,
            vec![
                NewMessage {
                    turn_id: "t1".into(),
                    role: MessageRole::User,
                    author: Some("julius".into()),
                    source_session: None,
                    timestamp: at(5),
                    text: "pin the archive".into(),
                    tools: vec![],
                    files: vec![],
                    parent: None,
                    blocks: Vec::new(),
                },
                NewMessage {
                    turn_id: "t2".into(),
                    role: MessageRole::Assistant,
                    author: Some("pcx-agent".into()),
                    source_session: None,
                    timestamp: at(6),
                    text: String::new(),
                    tools: vec!["fs_read".into()],
                    files: vec![],
                    parent: Some("t1".into()),
                    blocks: vec![MessageBlock::ToolUse {
                        id: "c1".into(),
                        name: "fs_read".into(),
                        input: serde_json::json!({ "path": "README.md" }),
                    }],
                },
            ],
        )
        .await
        .unwrap();

    stores
        .cards
        .restore_card_versions(
            CardScope::Workspace(wid),
            vec![Card {
                scope: CardScope::Workspace(wid),
                version: 1,
                text: "# golden\n".into(),
                updated_at: at(7),
                author: Some("claude-code".into()),
            }],
            true,
        )
        .await
        .unwrap();

    let task = |title: &str| DocumentDraft {
        workspace_id: wid,
        id: id(4),
        kind: "board/task".parse().unwrap(),
        data: DocumentData::Message {
            type_name: "pcx.board.v1.Task".into(),
            bytes: title.as_bytes().to_vec(),
        },
        title: title.to_string(),
        text: "pinned".into(),
        author: Some("board".into()),
        keys: Vec::new(),
    };
    stores
        .documents
        .restore_document_versions(
            wid,
            id(4),
            vec![
                task("pin").into_version(1, at(8)).0,
                task("pin it").into_version(2, at(9)).0,
            ],
            true,
            None,
        )
        .await
        .unwrap();

    let options = ExportOptions {
        include_checkpoints: true,
        include_messages: true,
        embedding_model: Some(EmbeddingModelInfo {
            model: "PotionMultilingual".into(),
            dimension: 3,
        }),
        pretty: true,
        ..ExportOptions::default()
    };
    let ports = crate::capabilities::transfer::tests::ports_over(stores, storage, 3);
    let mut plan =
        crate::capabilities::transfer::export::plan(&ports, &ExportScope::Workspace(wid), options)
            .await
            .unwrap();
    plan.header.metadata.exported_at = at(100);
    let out = crate::capabilities::transfer::tests::Shared::default();
    let size = plan.write(Box::new(out.clone())).await.unwrap();
    let bytes = out.bytes();
    assert_eq!(size.written_bytes, bytes.len() as u64);
    bytes
}

#[tokio::test]
async fn export_1_10_0_bytes_are_pinned() {
    let archive = String::from_utf8(golden_archive().await).unwrap();
    if std::env::var_os("PCX_WRITE_GOLDEN").is_some() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/archive-1.10.0.json"
        );
        std::fs::write(path, &archive).unwrap();
    }
    assert_eq!(archive, GOLDEN);
}

/// The 1.4.1 archive of the same content imports into a fresh store.
#[tokio::test]
async fn an_archive_of_1_4_1_still_imports() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Arc::new(RealRocksDBStorage::new(dir.path()).await.unwrap());
    let stores = Stores::of(Arc::clone(&storage));
    let ports = crate::capabilities::transfer::tests::ports_over(stores.clone(), storage, 3);
    let data = crate::capabilities::transfer::tests::decode(GOLDEN_1_4_1.as_bytes()).unwrap();
    assert_eq!(data.format_version, "1.4.1");
    let result =
        crate::capabilities::transfer::tests::import(&ports, data, &ImportOptions::default())
            .await
            .unwrap();
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(
        (
            result.sessions_imported,
            result.workspaces_imported,
            result.cards_imported
        ),
        (1, 1, 1)
    );
    assert_eq!(result.documents_imported, 0);
    assert!(stores.sessions.session_exists(id(1)).await.unwrap());
}

/// The 1.5.0 archive of the same content (its messages without parents
/// or blocks) imports into a fresh store.
#[tokio::test]
async fn an_archive_of_1_5_0_still_imports() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Arc::new(RealRocksDBStorage::new(dir.path()).await.unwrap());
    let stores = Stores::of(Arc::clone(&storage));
    let ports = crate::capabilities::transfer::tests::ports_over(stores.clone(), storage, 3);
    let data = crate::capabilities::transfer::tests::decode(GOLDEN_1_5_0.as_bytes()).unwrap();
    assert_eq!(data.format_version, "1.5.0");
    let result =
        crate::capabilities::transfer::tests::import(&ports, data, &ImportOptions::default())
            .await
            .unwrap();
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(
        (result.sessions_imported, result.documents_imported),
        (1, 1)
    );
    assert!(stores.sessions.session_exists(id(1)).await.unwrap());
}

/// The 1.6.0 archive of the same content (its document's data as JSON)
/// imports into a fresh store, the data kept as JSON until the migration.
#[tokio::test]
async fn an_archive_of_1_6_0_still_imports() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Arc::new(RealRocksDBStorage::new(dir.path()).await.unwrap());
    let stores = Stores::of(Arc::clone(&storage));
    let ports = crate::capabilities::transfer::tests::ports_over(stores.clone(), storage, 3);
    let data = crate::capabilities::transfer::tests::decode(GOLDEN_1_6_0.as_bytes()).unwrap();
    assert_eq!(data.format_version, "1.6.0");
    let result =
        crate::capabilities::transfer::tests::import(&ports, data, &ImportOptions::default())
            .await
            .unwrap();
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(result.documents_imported, 1);
    let document = stores
        .documents
        .get_document(id(2), id(4), None)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(document.data, DocumentData::Json(_)));
}

/// The 1.7.0 archive of the same content (its workspace without a slug)
/// imports into a fresh store, the workspace's slug made from its name.
#[tokio::test]
async fn an_archive_of_1_7_0_still_imports() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Arc::new(RealRocksDBStorage::new(dir.path()).await.unwrap());
    let stores = Stores::of(Arc::clone(&storage));
    let ports = crate::capabilities::transfer::tests::ports_over(stores.clone(), storage, 3);
    let data = crate::capabilities::transfer::tests::decode(GOLDEN_1_7_0.as_bytes()).unwrap();
    assert_eq!(data.format_version, "1.7.0");
    assert_eq!(data.workspaces[0].slug, None);
    let name = data.workspaces[0].name.clone();
    let result =
        crate::capabilities::transfer::tests::import(&ports, data, &ImportOptions::default())
            .await
            .unwrap();
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    let imported = stores.workspaces.list_workspaces().await.unwrap();
    assert_eq!(
        imported[0].slug,
        post_cortex_core::slug::slug_base(&name, "workspace")
    );
}

/// The 1.8.0 archive of the same content (its workspace before remote
/// reranking) imports into a fresh store, the remote reranker forbidden.
#[tokio::test]
async fn an_archive_of_1_8_0_still_imports() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Arc::new(RealRocksDBStorage::new(dir.path()).await.unwrap());
    let stores = Stores::of(Arc::clone(&storage));
    let ports = crate::capabilities::transfer::tests::ports_over(stores.clone(), storage, 3);
    let data = crate::capabilities::transfer::tests::decode(GOLDEN_1_8_0.as_bytes()).unwrap();
    assert_eq!(data.format_version, "1.8.0");
    assert!(!data.workspaces[0].remote_rerank);
    let result =
        crate::capabilities::transfer::tests::import(&ports, data, &ImportOptions::default())
            .await
            .unwrap();
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    let imported = stores.workspaces.list_workspaces().await.unwrap();
    assert_eq!(imported[0].slug, "golden-ws");
    assert!(!imported[0].remote_rerank);
}

/// The 1.9.0 archive of the same content (before persons' user cards)
/// imports into a fresh store.
#[tokio::test]
async fn an_archive_of_1_9_0_still_imports() {
    let dir = tempfile::tempdir().unwrap();
    let storage = Arc::new(RealRocksDBStorage::new(dir.path()).await.unwrap());
    let stores = Stores::of(Arc::clone(&storage));
    let ports = crate::capabilities::transfer::tests::ports_over(stores.clone(), storage, 3);
    let data = crate::capabilities::transfer::tests::decode(GOLDEN_1_9_0.as_bytes()).unwrap();
    assert_eq!(data.format_version, "1.9.0");
    let result =
        crate::capabilities::transfer::tests::import(&ports, data, &ImportOptions::default())
            .await
            .unwrap();
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert!(stores.sessions.session_exists(id(1)).await.unwrap());
}
