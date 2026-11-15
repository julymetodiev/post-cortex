// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Export and import over gRPC, through a real server since they stream.

use super::lifecycle::{serve, system};
use super::*;
use post_cortex_storage::RealRocksDBStorage;
use std::time::Duration;

/// Export from one daemon in chunks and import the bytes into a fresh one:
/// the preview counts, then the committed counts, equal the export's; a
/// second preview reports the conflicts.
#[tokio::test]
async fn export_import_round_trip_over_grpc() {
    let dir = tempfile::tempdir().unwrap();
    let source = system(
        &RealRocksDBStorage::new(dir.path().join("src"))
            .await
            .unwrap(),
    )
    .await;
    let target = system(
        &RealRocksDBStorage::new(dir.path().join("dst"))
            .await
            .unwrap(),
    )
    .await;
    let ws = source
        .workspaces
        .create(post_cortex_memory::capabilities::CreateWorkspace {
            name: "ws".into(),
            description: "d".into(),
        })
        .await
        .unwrap()
        .id;
    let mut sessions = Vec::new();
    for name in ["a", "b"] {
        let sid = source
            .sessions
            .create(Some(name.into()), None)
            .await
            .unwrap();
        for i in 0..2 {
            source
                .records
                .write(super::lifecycle::record(sid, &format!("{name} {i}")))
                .await
                .unwrap();
        }
        sessions.push(sid);
    }
    source
        .workspaces
        .add_session(
            ws,
            sessions[0],
            post_cortex_core::workspace::SessionRole::Primary,
        )
        .await
        .unwrap();
    source.workspaces.set_archived(ws, true).await.unwrap();
    let turn = |id: &str| post_cortex_core::core::message::NewMessage {
        turn_id: id.into(),
        role: post_cortex_core::core::message::MessageRole::User,
        author: None,
        source_session: None,
        timestamp: chrono::Utc::now(),
        text: format!("turn {id}"),
        tools: vec![],
        files: vec![],
        parent: None,
        blocks: Vec::new(),
    };
    source
        .messages
        .append(post_cortex_core::services::AppendMessagesRequest {
            session_id: sessions[0],
            messages: vec![turn("t1"), turn("t2")],
        })
        .await
        .unwrap();
    put_card(
        &source,
        post_cortex_core::core::card::CardScope::Workspace(ws),
        "ws card",
        0,
    )
    .await;

    let mut from = serve(source.clone()).await;
    let mut to = serve(target.clone()).await;

    let bad = from
        .export(ExportRequest {
            scope: "galaxy".into(),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(bad.code(), tonic::Code::InvalidArgument);

    for compression in ["", "gzip", "zstd"] {
        let mut stream = from
            .export(ExportRequest {
                compression: compression.into(),
                ..Default::default()
            })
            .await
            .unwrap()
            .into_inner();
        let mut bytes = Vec::new();
        let mut summary = None;
        while let Some(chunk) = stream.message().await.unwrap() {
            bytes.extend_from_slice(&chunk.data);
            if chunk.last {
                summary = chunk.summary;
            }
        }
        let summary = summary.expect("the last chunk has the summary");
        assert_eq!(summary.total_bytes, bytes.len() as u64);
        assert_eq!(
            (summary.sessions, summary.workspaces, summary.updates),
            (2, 1, 4)
        );
        assert_eq!((summary.messages, summary.cards), (2, 1));
        // The bytes are a `pcx export` file.
        let mut parts = Vec::new();
        post_cortex_ports::ArchiveCodec::read(
            &post_cortex_storage::JsonArchive,
            &mut &bytes[..],
            &mut |part| {
                parts.push(part);
                std::ops::ControlFlow::Continue(())
            },
        )
        .unwrap();
        let data = post_cortex_core::archive::ExportData::from_parts(parts);
        assert_eq!(data.sessions.len(), 2);

        // Sent in small chunks, settings on the first one only.
        let chunks = |commit: bool| {
            let mut out: Vec<ImportChunk> = bytes
                .chunks(1000)
                .map(|data| ImportChunk {
                    data: data.to_vec(),
                    settings: None,
                })
                .collect();
            out[0].settings = Some(ImportSettings {
                commit,
                skip_existing: true,
                ..Default::default()
            });
            futures::stream::iter(out)
        };
        let preview = to.import(chunks(false)).await.unwrap().into_inner();
        assert!(!preview.committed);
        assert_eq!(
            (preview.sessions, preview.workspaces, preview.updates),
            (summary.sessions, summary.workspaces, summary.updates)
        );
        assert_eq!((preview.messages, preview.cards), (2, 1));
        if compression.is_empty() {
            assert_eq!(preview.messages_new, 2);
            assert!(preview.conflicts.is_empty(), "{:?}", preview.conflicts);
            assert!(
                target.sessions.list().await.unwrap().is_empty(),
                "a preview writes nothing"
            );
            let done = to.import(chunks(true)).await.unwrap().into_inner();
            assert!(
                done.committed && done.errors.is_empty(),
                "{:?}",
                done.errors
            );
            assert_eq!(
                (
                    done.sessions_imported,
                    done.workspaces_imported,
                    done.updates_imported
                ),
                (2, 1, 4)
            );
            assert_eq!((done.messages_imported, done.cards_imported), (2, 1));
        } else {
            // Imported already: every session and the workspace conflict;
            // the messages and the card are there already.
            assert_eq!(preview.messages_new, 0);
            assert_eq!(preview.conflicts.len(), 3);
            assert!(preview.conflicts.iter().all(|c| c.resolution == "skip"));
        }
    }

    // Diverged cards: the target's own v2, then the source's v2 written
    // later. The target's stays unless cards_newer asks for the later one.
    let ws_card = post_cortex_core::core::card::CardScope::Workspace(ws);
    put_card(&target, ws_card, "mine\nmore", 1).await;
    tokio::time::sleep(Duration::from_millis(1100)).await;
    put_card(&source, ws_card, "theirs, later", 1).await;
    let archive = {
        let mut stream = from
            .export(ExportRequest::default())
            .await
            .unwrap()
            .into_inner();
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.message().await.unwrap() {
            bytes.extend_from_slice(&chunk.data);
        }
        bytes
    };
    let send = |settings: ImportSettings| {
        futures::stream::iter(vec![ImportChunk {
            data: archive.clone(),
            settings: Some(settings),
        }])
    };
    let skip = ImportSettings {
        skip_existing: true,
        ..Default::default()
    };
    let newer = ImportSettings {
        cards_newer: true,
        ..skip.clone()
    };
    let preview = to.import(send(skip.clone())).await.unwrap().into_inner();
    let card = preview.conflicts.iter().find(|c| c.kind == "card").unwrap();
    assert_eq!(card.resolution, "keep");
    let (here, there) = (card.target.clone().unwrap(), card.archive.clone().unwrap());
    assert_eq!((here.version, here.first_line.as_str()), (2, "mine"));
    assert_eq!(
        (there.version, there.first_line.as_str()),
        (2, "theirs, later")
    );
    assert!(there.updated_at_unix > here.updated_at_unix);
    let kept = to
        .import(send(ImportSettings {
            commit: true,
            ..skip.clone()
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!((kept.cards_imported, kept.cards_skipped), (0, 1));
    let latest = || async {
        target
            .cards
            .get(post_cortex_core::services::GetCardRequest {
                scope: ws_card,
                history: 0,
            })
            .await
            .unwrap()
            .card
            .unwrap()
    };
    assert_eq!(latest().await.text, "mine\nmore");
    let preview = to.import(send(newer.clone())).await.unwrap().into_inner();
    assert!(
        preview
            .conflicts
            .iter()
            .any(|c| c.kind == "card" && c.resolution == "update")
    );
    let taken = to
        .import(send(ImportSettings {
            commit: true,
            ..newer
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(taken.cards_imported, 1);
    let head = latest().await;
    assert_eq!((head.version, head.text.as_str()), (3, "theirs, later"));

    let imported = target.workspaces.get(ws).ok().unwrap();
    assert!(imported.is_archived());
    let mut ids = target.sessions.list().await.unwrap();
    ids.sort();
    sessions.sort();
    assert_eq!(ids, sessions);
    let updates: usize = {
        let mut n = 0;
        for sid in &sessions {
            n += target.sessions.get(*sid).await.unwrap().records().len();
        }
        n
    };
    assert_eq!(updates, 4);

    let mut lean = from
        .export(ExportRequest {
            no_messages: true,
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner();
    let mut summary = None;
    while let Some(chunk) = lean.message().await.unwrap() {
        if chunk.last {
            summary = chunk.summary;
        }
    }
    let summary = summary.unwrap();
    // The workspace card has two versions by now.
    assert_eq!((summary.messages, summary.cards), (0, 2));

    let garbage = to
        .import(futures::stream::iter(vec![ImportChunk {
            data: b"not json".to_vec(),
            settings: None,
        }]))
        .await
        .unwrap_err();
    assert_eq!(garbage.code(), tonic::Code::InvalidArgument);
}

/// Write the next version of a card (the version before is
/// `expected_version`).
async fn put_card(
    memory: &post_cortex_memory::Memory,
    scope: post_cortex_core::core::card::CardScope,
    text: &str,
    expected_version: u64,
) {
    let written = memory
        .cards
        .update(post_cortex_core::services::UpdateCardRequest {
            scope,
            text: text.to_string(),
            expected_version,
            author: None,
        })
        .await
        .unwrap();
    assert!(written.written);
}

/// An archive larger than the configured limit is refused while it is
/// spooled; an export of a workspace that does not exist fails on the call,
/// before any chunk.
#[tokio::test]
async fn the_import_limit_and_an_unknown_export_scope_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let memory = Arc::new(
        crate::memory_tests::over_rocksdb(
            RealRocksDBStorage::new(dir.path()).await.unwrap(),
            post_cortex_memory::SystemConfig {
                enable_embeddings: false,
                max_import_bytes: 10,
                ..Default::default()
            },
        )
        .await
        .unwrap(),
    );
    let mut client = serve(memory).await;
    let chunk = |data: &[u8]| ImportChunk {
        data: data.to_vec(),
        settings: Some(ImportSettings::default()),
    };
    let refused = client
        .import(futures::stream::iter(vec![chunk(&[b'{'; 20])]))
        .await
        .unwrap_err();
    assert_eq!(refused.code(), tonic::Code::InvalidArgument);
    assert!(
        refused.message().contains("larger than 10 bytes"),
        "{refused:?}"
    );
    let empty = client
        .import(futures::stream::iter(vec![chunk(&[])]))
        .await
        .unwrap_err();
    assert_eq!(empty.code(), tonic::Code::InvalidArgument);

    let missing = client
        .export(ExportRequest {
            scope: "workspace".into(),
            workspace_id: uuid::Uuid::new_v4().to_string(),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(missing.code(), tonic::Code::NotFound);
}
