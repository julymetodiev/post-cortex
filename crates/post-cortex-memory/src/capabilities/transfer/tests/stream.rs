// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Export and import stream: the spool keeps its limit, a cut archive is
//! refused by the preview pass before anything is written, and the counts
//! the header carries are those of the archive's content.

use std::sync::Arc;

use crate::capabilities::transfer::parts::PartReader;
use crate::capabilities::transfer::preview::scan;
use crate::capabilities::transfer::tests::{encode, export, of};
use crate::capabilities::transfer::*;
use post_cortex_core::archive::CompressionType;
use post_cortex_core::core::message::{MessageRole, NewMessage};
use post_cortex_core::session::active_session::ActiveSession;
use post_cortex_ports::conformance::record;
use post_cortex_ports::storage::{MessageStore, SessionChanges, SessionStore};
use post_cortex_storage::RealRocksDBStorage;
use uuid::Uuid;

async fn store(dir: &tempfile::TempDir, name: &str) -> RealRocksDBStorage {
    RealRocksDBStorage::new(dir.path().join(name))
        .await
        .unwrap()
}

/// A store with two sessions, each with a record and a message.
async fn source(dir: &tempfile::TempDir) -> RealRocksDBStorage {
    let storage = store(dir, "src").await;
    for n in 0..2 {
        let mut session = ActiveSession::new(
            Uuid::new_v4(),
            Some(format!("s{n}")),
            None,
            chrono::Utc::now(),
        );
        session.apply_record(record(n + 1, "stored", &["pcx"]));
        storage
            .save_session(&session, SessionChanges::everything(&session, &[]))
            .await
            .unwrap();
        storage
            .append_messages(
                session.id(),
                vec![NewMessage {
                    turn_id: format!("t{n}"),
                    role: MessageRole::User,
                    author: None,
                    source_session: None,
                    timestamp: chrono::Utc::now(),
                    text: "hello".into(),
                    tools: vec![],
                    files: vec![],
                    parent: None,
                    blocks: Vec::new(),
                }],
            )
            .await
            .unwrap();
    }
    storage
}

#[tokio::test]
async fn the_spool_keeps_its_limit_and_refuses_nothing_received() {
    let mut spool = ArchiveSpool::new(4).unwrap();
    spool.append(b"{\"a\"").await.unwrap();
    assert!(matches!(
        spool.append(b"x").await,
        Err(TransferError::TooLarge(4))
    ));
    let empty = ArchiveSpool::new(4).unwrap();
    assert!(matches!(empty.finish().await, Err(TransferError::Empty)));
}

#[tokio::test]
async fn the_header_counts_what_the_archive_holds() {
    let dir = tempfile::tempdir().unwrap();
    let source = source(&dir).await;
    let archive = export(&of(&source), &ExportScope::All, &ExportOptions::default())
        .await
        .unwrap();
    let mut recounted = archive.clone();
    recounted.update_counts();
    let (header, recount) = (&archive.metadata, &recounted.metadata);
    assert_eq!(header.session_count, 2);
    assert_eq!(
        (
            header.update_count,
            header.message_count,
            header.embedding_count,
            header.card_count
        ),
        (
            recount.update_count,
            recount.message_count,
            recount.embedding_count,
            recount.card_count
        )
    );
}

/// The preview pass reads the whole archive first, so a cut one is refused
/// with nothing written.
#[tokio::test]
async fn a_cut_archive_is_refused_before_anything_is_written() {
    let dir = tempfile::tempdir().unwrap();
    let source = source(&dir).await;
    let archive = export(&of(&source), &ExportScope::All, &ExportOptions::default())
        .await
        .unwrap();
    let bytes = encode(&archive, CompressionType::Gzip, false);
    let mut spool = ArchiveSpool::new(u64::MAX).unwrap();
    spool.append(&bytes[..bytes.len() - 20]).await.unwrap();
    let file = spool.finish().await.unwrap();

    let target = store(&dir, "dst").await;
    let ports = of(&target);
    let parts = PartReader::start(Arc::clone(&ports.codec), &file).unwrap();
    let refused = scan(
        &ports.stores,
        ports.codec.as_ref(),
        parts,
        &ImportOptions::default(),
    )
    .await;
    assert!(
        matches!(refused, Err(TransferError::Archive(_))),
        "{refused:?}"
    );
    assert!(target.list_sessions().await.unwrap().is_empty());
}
