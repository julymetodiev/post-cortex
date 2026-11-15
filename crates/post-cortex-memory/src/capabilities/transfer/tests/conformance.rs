// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Import: `--overwrite` replaces a session as it replaces a workspace, and
//! an imported workspace keeps its creation time and member roles.

use crate::capabilities::transfer::tests::{
    decode, encode, export, import, of_adapter, preview_of,
};
use crate::capabilities::transfer::*;
use post_cortex_core::archive::*;
use post_cortex_core::core::card::CardScope;
use post_cortex_core::core::dismissal::{RadarDismissal, RadarItemKind};
use post_cortex_core::core::message::{MessageRole, NewMessage};
use post_cortex_core::session::active_session::ActiveSession;
use post_cortex_core::workspace::SessionRole;
use post_cortex_ports::storage::Stores;
use post_cortex_ports::{MESSAGE_CONTENT_TYPE, VectorMetadata};
use uuid::Uuid;

use post_cortex_ports::conformance::checkpoints::checkpoint;
use post_cortex_ports::conformance::{at, record};

use crate::capabilities::transfer::tests::on_backends;
use post_cortex_ports::conformance::Adapter;
use post_cortex_ports::storage::StoredWorkspace;

fn overwrite() -> ImportOptions {
    ImportOptions {
        overwrite: true,
        ..Default::default()
    }
}

/// Importing a session with `--overwrite` leaves the archived session:
/// records the target gained since are gone, not merged.
async fn overwrite_replaces_the_session(adapter: &Adapter) {
    let stores: &Stores = adapter;
    let ports = of_adapter(adapter, 16);
    let mut session = ActiveSession::new(Uuid::new_v4(), Some("s".into()), None, at(0));
    session.apply_record(record(1, "archived", &["A", "B"]));
    stores
        .sessions
        .save_session(
            &session,
            post_cortex_ports::storage::SessionChanges::everything(&session, &[]),
        )
        .await
        .unwrap();
    let archive = export(
        &ports,
        &ExportScope::Sessions(vec![session.id()]),
        &ExportOptions::default(),
    )
    .await
    .unwrap();

    let mut later = session.clone();
    let extra = record(2, "only in the target", &["C"]);
    later.apply_record(extra.clone());
    stores
        .sessions
        .save_session(
            &later,
            post_cortex_ports::storage::SessionChanges {
                updates: vec![extra.clone()],
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let result = import(&ports, archive, &overwrite()).await.unwrap();
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    let loaded = stores
        .sessions
        .load_session(session.id())
        .await
        .unwrap()
        .session;
    let ids: Vec<Uuid> = loaded.records().iter().map(|u| u.id).collect();
    assert_eq!(ids, [session.records()[0].id]);
    assert!(!loaded.entity_graph().has_entity("C"), "graph merged");
}

/// An imported workspace keeps its creation time, member roles and slug, on
/// create and on `--overwrite`; an overwrite keeps the target's card. A
/// slug another workspace has, or none (an older archive), becomes the one
/// a new workspace of its name gets.
async fn import_keeps_the_workspace(adapter: &Adapter) {
    let stores: &Stores = adapter;
    let ports = of_adapter(adapter, 16);
    let (id, member) = (Uuid::new_v4(), Uuid::new_v4());
    stores
        .sessions
        .save_session(
            &ActiveSession::new(member, None, None, at(0)),
            post_cortex_ports::storage::SessionChanges::default(),
        )
        .await
        .unwrap();
    stores
        .workspaces
        .restore_workspace(&StoredWorkspace {
            id,
            name: "ws".into(),
            slug: "kept".into(),
            description: "d".into(),
            sessions: vec![(member, SessionRole::Dependency)],
            created_at: 1_000,
            projects: vec![],
            archived_at: None,
            remote_rerank: false,
        })
        .await
        .unwrap();
    let archive = export(
        &ports,
        &ExportScope::Workspace(id),
        &ExportOptions::default(),
    )
    .await
    .unwrap();

    // Into an empty store.
    let (other, _keep) = crate::capabilities::transfer::tests::rocksdb().await;
    let result = import(
        &of_adapter(&other, 16),
        archive.clone(),
        &ImportOptions::default(),
    )
    .await
    .unwrap();
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    let imported = other.workspaces.list_workspaces().await.unwrap();
    assert_eq!(imported[0].created_at, 1_000);
    assert_eq!(imported[0].slug, "kept");
    assert_eq!(imported[0].sessions, [(member, SessionRole::Dependency)]);

    // Into a store where another workspace has its slug, then from an
    // archive older than slugs.
    let hex = id.simple().to_string();
    // Its own slug taken: the slug of its name ("ws"); none and its name's
    // taken: that with its id.
    for (slug, holder, want) in [
        (Some("kept"), "kept", "ws".to_string()),
        (None, "ws", format!("ws-{}", &hex[..6])),
    ] {
        let (other, _keep) = crate::capabilities::transfer::tests::rocksdb().await;
        let holder_id = Uuid::new_v4();
        other
            .workspaces
            .save_workspace_metadata(holder_id, "holder", holder, "", &[])
            .await
            .unwrap();
        let mut file = archive.clone();
        file.workspaces[0].slug = slug.map(Into::into);
        let result = import(&of_adapter(&other, 16), file, &ImportOptions::default())
            .await
            .unwrap();
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        let imported = other.workspaces.list_workspaces().await.unwrap();
        let ws = imported.iter().find(|w| w.id == id).unwrap();
        assert_eq!(ws.slug, want);
    }

    // Over the original, renamed and with a card since.
    stores
        .workspaces
        .save_workspace_metadata(id, "renamed", &id.to_string(), "", &[])
        .await
        .unwrap();
    stores
        .cards
        .put_card(CardScope::Workspace(id), "goals".into(), None, 0)
        .await
        .unwrap();
    let result = import(&ports, archive, &overwrite()).await.unwrap();
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    let ws = stores
        .workspaces
        .list_workspaces()
        .await
        .unwrap()
        .into_iter()
        .find(|w| w.id == id)
        .unwrap();
    assert_eq!((ws.name.as_str(), ws.created_at), ("ws", 1_000));
    assert_eq!(ws.sessions, [(member, SessionRole::Dependency)]);
    assert!(
        stores
            .cards
            .get_card(CardScope::Workspace(id))
            .await
            .unwrap()
            .is_some(),
        "the overwrite deleted the target's card"
    );
}

/// A session in a workspace with everything an archive carries: a record
/// with its vector, a message with its vector, a checkpoint, a radar
/// dismissal and a card. Returns (session, workspace).
async fn furnished(adapter: &Adapter) -> (Uuid, Uuid) {
    let stores: &Stores = adapter;
    let mut session = ActiveSession::new(Uuid::new_v4(), Some("s".into()), None, at(0));
    let record = session.apply_record(record(1, "decided", &["A", "B"]));
    let sid = session.id();
    stores
        .sessions
        .save_session(
            &session,
            post_cortex_ports::storage::SessionChanges::everything(&session, &[]),
        )
        .await
        .unwrap();
    let meta = |id: &str, content_type: &str| VectorMetadata {
        id: id.into(),
        text: "t".into(),
        source: sid.to_string(),
        content_type: content_type.into(),
        timestamp: at(1),
        metadata: Default::default(),
    };
    adapter
        .vectors
        .upsert_vectors(vec![
            (
                vec![0.25; 16],
                meta(&record.id.to_string(), "UpdateContent"),
            ),
            (vec![0.5; 16], meta("message:0", MESSAGE_CONTENT_TYPE)),
        ])
        .await
        .unwrap();
    stores
        .messages
        .append_messages(
            sid,
            vec![NewMessage {
                turn_id: "t1".into(),
                role: MessageRole::User,
                author: None,
                source_session: None,
                timestamp: at(2),
                text: "hello".into(),
                tools: vec![],
                files: vec![],
                parent: None,
                blocks: Vec::new(),
            }],
        )
        .await
        .unwrap();
    stores
        .checkpoints
        .save_checkpoint(&checkpoint(sid))
        .await
        .unwrap();
    let ws = Uuid::new_v4();
    stores
        .workspaces
        .restore_workspace(&StoredWorkspace {
            id: ws,
            name: "ws".into(),
            slug: ws.to_string(),
            description: String::new(),
            sessions: vec![(sid, SessionRole::Primary)],
            created_at: 1_000,
            projects: vec![],
            archived_at: None,
            remote_rerank: false,
        })
        .await
        .unwrap();
    stores
        .dismissals
        .put_dismissal(ws, &dismissal())
        .await
        .unwrap();
    stores
        .cards
        .put_card(CardScope::Workspace(ws), "goals".into(), None, 0)
        .await
        .unwrap();
    (sid, ws)
}

fn dismissal() -> RadarDismissal {
    RadarDismissal {
        kind: RadarItemKind::ALL[0],
        key: "stale-thing".into(),
        dismissed_at: at(5),
        author: None,
    }
}

/// Format 1.4.0: every scope carries the same sections for what it
/// exports (records once, the vectors of records and messages, messages,
/// checkpoints; a workspace's dismissals and card), whichever backend
/// wrote it. Before, RocksDB exported only message vectors, SurrealDB all
/// of them, every record was written twice and dismissals not at all.
async fn every_scope_carries_the_same_sections(adapter: &Adapter) {
    let _stores: &Stores = adapter;
    let ports = of_adapter(adapter, 16);
    let (sid, ws) = furnished(adapter).await;
    let options = ExportOptions {
        include_checkpoints: true,
        ..Default::default()
    };
    for scope in [
        ExportScope::All,
        ExportScope::Sessions(vec![sid]),
        ExportScope::Workspace(ws),
    ] {
        let archive = export(&ports, &scope, &options).await.unwrap();
        assert_eq!(
            archive.format_version,
            post_cortex_storage::export_import::EXPORT_FORMAT_VERSION
        );
        let session = archive
            .sessions
            .iter()
            .find(|s| s.session.session.id() == sid)
            .unwrap();
        assert!(session.updates.is_empty(), "{scope:?}: records twice");
        assert_eq!(session.session.session.records().len(), 1, "{scope:?}");
        assert_eq!(archive.metadata.update_count, 1, "{scope:?}");
        assert_eq!(session.messages.len(), 1, "{scope:?}");
        let mut vectors: Vec<&str> = archive
            .embeddings
            .iter()
            .filter(|e| e.session_id == sid.to_string())
            .map(|e| e.content_type.as_str())
            .collect();
        vectors.sort();
        assert_eq!(
            vectors,
            [MESSAGE_CONTENT_TYPE, "UpdateContent"],
            "{scope:?}"
        );
        assert_eq!(archive.checkpoints.len(), 1, "{scope:?}");
        if !matches!(scope, ExportScope::Sessions(_)) {
            let workspace = archive.workspaces.iter().find(|w| w.id == ws).unwrap();
            assert_eq!(workspace.dismissals, [dismissal()], "{scope:?}");
            assert!(
                archive
                    .cards
                    .iter()
                    .any(|c| c.scope == CardScope::Workspace(ws)),
                "{scope:?}"
            );
        }
        // The file reads back with the records once.
        let read = decode(&encode(&archive, CompressionType::None, false)).unwrap();
        let session = read
            .sessions
            .iter()
            .find(|s| s.session.session.id() == sid)
            .unwrap();
        assert_eq!(session.session.session.records().len(), 1);
    }
}

/// An archive goes into an empty store with every section: the record, its
/// vectors, the message, the checkpoint, the dismissal and the card.
async fn an_archive_restores_every_section(adapter: &Adapter) {
    let _stores: &Stores = adapter;
    let ports = of_adapter(adapter, 16);
    let (sid, ws) = furnished(adapter).await;
    let options = ExportOptions {
        include_checkpoints: true,
        ..Default::default()
    };
    let archive = export(&ports, &ExportScope::All, &options).await.unwrap();
    let (target, _keep) = crate::capabilities::transfer::tests::rocksdb().await;
    let to = &target;
    let result = import(&of_adapter(to, 16), archive, &ImportOptions::default())
        .await
        .unwrap();
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(
        to.sessions
            .load_session(sid)
            .await
            .unwrap()
            .session
            .records()
            .len(),
        1
    );
    assert_eq!(
        adapter
            .vectors
            .count_session_embeddings(&sid.to_string())
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        to.messages.load_messages(sid, 0..10).await.unwrap().len(),
        1
    );
    assert_eq!(to.checkpoints.list_checkpoints(sid).await.unwrap().len(), 1);
    assert_eq!(
        to.dismissals.list_dismissals(ws).await.unwrap(),
        [dismissal()]
    );
    assert!(
        to.cards
            .get_card(CardScope::Workspace(ws))
            .await
            .unwrap()
            .is_some()
    );
}

/// One version gate on every backend: 1.x up to 1.4 imports (1.2.7 failed
/// on SurrealDB, which listed exact versions), newer or malformed versions
/// fail before anything is written.
async fn one_version_gate(adapter: &Adapter) {
    let _stores: &Stores = adapter;
    let ports = of_adapter(adapter, 16);
    let (sid, _) = furnished(adapter).await;
    let archive = export(
        &ports,
        &ExportScope::Sessions(vec![sid]),
        &ExportOptions::default(),
    )
    .await
    .unwrap();
    for (version, readable) in [
        ("1.2.7", true),
        ("1.0.0", true),
        ("1.4.0", true),
        ("1.5.0", true),
        ("1.6.0", true),
        ("1.7.0", true),
        ("1.8.0", true),
        ("1.9.0", true),
        ("1.10.0", true),
        ("1.11.0", false),
        ("2.0.0", false),
        ("1.x", false),
        ("1.4", false),
    ] {
        let mut file = archive.clone();
        file.format_version = version.into();
        let (target, _keep) = crate::capabilities::transfer::tests::rocksdb().await;
        let imported = import(
            &of_adapter(&target, 16),
            file.clone(),
            &ImportOptions::default(),
        )
        .await;
        let previewed =
            preview_of(&of_adapter(&target, 16), &file, &ImportOptions::default()).await;
        assert_eq!(imported.is_ok(), readable, "import {version}");
        assert_eq!(previewed.is_ok(), readable, "preview {version}");
        let stored = target.sessions.list_sessions().await.unwrap();
        assert_eq!(!stored.is_empty(), readable, "{version} wrote");
    }
}

on_backends!(
    overwrite_replaces_the_session,
    import_keeps_the_workspace,
    every_scope_carries_the_same_sections,
    an_archive_restores_every_section,
    one_version_gate,
);
