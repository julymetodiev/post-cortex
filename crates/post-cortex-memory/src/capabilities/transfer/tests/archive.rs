// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Archives and cards/messages exports round-trip between backends.

use crate::capabilities::transfer::tests::Backend;
use crate::capabilities::transfer::tests::merge_lifecycle::history;
use crate::capabilities::transfer::tests::merge_lifecycle::turn;
use crate::capabilities::transfer::tests::merge_lifecycle::write_card;
use crate::capabilities::transfer::tests::{export, import, of, preview_of};
use crate::capabilities::transfer::*;
use chrono::Utc;
use post_cortex_core::core::card::CardScope;
use post_cortex_core::core::context_update::{ContextUpdate, UpdateContent, UpdateType};
use post_cortex_core::core::message::{Message, MessageRole, NewMessage};
use post_cortex_core::session::active_session::ActiveSession;
use post_cortex_core::workspace::SessionRole;
use post_cortex_ports::storage::{MessageStore, WorkspaceUpdate};
use post_cortex_ports::{MESSAGE_CONTENT_TYPE, VectorMetadata};
use uuid::Uuid;

/// What `export_archive` writes, `import_archive` reads back into an empty
/// store; an archived workspace stays archived.
pub async fn archives_round_trip(source: &impl Backend, target: &impl Backend) {
    let session = ActiveSession::new(
        Uuid::new_v4(),
        Some("s".to_string()),
        None,
        chrono::Utc::now(),
    );
    source
        .save_session(
            &session,
            post_cortex_ports::storage::SessionChanges::everything(&session, &[]),
        )
        .await
        .unwrap();
    let workspace = Uuid::new_v4();
    source
        .save_workspace_metadata(workspace, "ws", &workspace.to_string(), "d", &[])
        .await
        .unwrap();
    source
        .add_session_to_workspace(workspace, session.id(), SessionRole::Primary)
        .await
        .unwrap();
    source
        .update_workspace(
            workspace,
            &WorkspaceUpdate {
                archived_at: Some(Some(7)),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let options = ExportOptions::default();
    let all = export(&of(source), &ExportScope::All, &options)
        .await
        .unwrap();
    assert_eq!(
        (all.sessions.len(), all.workspaces.len()),
        (1, 1),
        "whole store"
    );
    let one = export(&of(source), &ExportScope::Workspace(workspace), &options)
        .await
        .unwrap();
    assert_eq!(one.workspaces[0].archived_at, Some(7));
    let sessions = export(
        &of(source),
        &ExportScope::Sessions(vec![session.id()]),
        &options,
    )
    .await
    .unwrap();
    assert!(sessions.workspaces.is_empty());

    let result = import(&of(target), all, &ImportOptions::default())
        .await
        .unwrap();
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(
        (result.sessions_imported, result.workspaces_imported),
        (1, 1)
    );
    let ws = target.list_workspaces().await.unwrap();
    assert_eq!(ws.len(), 1);
    assert_eq!(ws[0].archived_at, Some(7));
    assert_eq!(ws[0].sessions, [(session.id(), SessionRole::Primary)]);
    assert!(target.session_exists(session.id()).await.unwrap());
}

pub fn record(title: &str) -> ContextUpdate {
    ContextUpdate {
        meta: Default::default(),
        id: Uuid::new_v4(),
        timestamp: Utc::now(),
        update_type: UpdateType::DecisionMade,
        content: UpdateContent {
            title: title.to_string(),
            description: String::new(),
            details: vec![],
            examples: vec![],
            implications: vec![],
        },
        related_code: None,
        parent_update: None,
        user_marked_important: false,
        creates_entities: vec![],
        creates_relationships: vec![],
        references_entities: vec![],
        typed_entities: vec![],
        author: None,
        derived_from: Vec::new(),
    }
}

pub async fn stream(storage: &impl MessageStore, session: Uuid) -> Vec<Message> {
    storage.load_messages(session, 0..u64::MAX).await.unwrap()
}

/// A message vector of `session` at `seq`, at the backends' dimension.
pub fn message_vector(session: Uuid, seq: u64) -> (Vec<f32>, VectorMetadata) {
    let mut vector = vec![0.0; 256];
    vector[seq as usize % 256] = 1.0;
    let mut metadata = VectorMetadata::new(
        format!("message:{seq}"),
        format!("turn {seq}"),
        session.to_string(),
        MESSAGE_CONTENT_TYPE.to_string(),
    );
    metadata.metadata.insert("seq".to_string(), seq.to_string());
    (vector, metadata)
}

/// Cards (the host's and a person's user card and a workspace's, with their
/// history and authors) and the
/// message stream (with its vectors) survive an export and an import into
/// an empty store; a second import writes nothing; an import into a store
/// that has the session merges the messages, also when the session is
/// skipped, and keeps its own diverged card; `include_messages: false` leaves the stream out;
/// and the scopes carry the cards they cover.
pub async fn cards_and_messages_round_trip(
    source: &impl Backend,
    fresh: &impl Backend,
    existing: &impl Backend,
) {
    let session = ActiveSession::new(
        Uuid::new_v4(),
        Some("s".to_string()),
        None,
        chrono::Utc::now(),
    );
    let sid = session.id();
    source
        .save_session(
            &session,
            post_cortex_ports::storage::SessionChanges {
                updates: vec![record("one"), record("two")],
                whole_graph: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let workspace = Uuid::new_v4();
    source
        .save_workspace_metadata(workspace, "ws", &workspace.to_string(), "d", &[])
        .await
        .unwrap();
    source
        .add_session_to_workspace(workspace, sid, SessionRole::Primary)
        .await
        .unwrap();
    let assistant = NewMessage {
        role: MessageRole::Assistant,
        author: Some("claude-code".to_string()),
        source_session: Some("cc-1".to_string()),
        tools: vec!["Edit".to_string()],
        files: vec!["src/lib.rs".to_string()],
        ..turn("b")
    };
    source
        .append_messages(sid, vec![turn("a"), assistant, turn("c")])
        .await
        .unwrap();
    let (vector, metadata) = message_vector(sid, 1);
    source
        .upsert_vectors(vec![(vector, metadata)])
        .await
        .unwrap();
    write_card(source, CardScope::User, "me", 0).await;
    let person = CardScope::Person(Uuid::new_v4());
    write_card(source, person, "mine alone", 0).await;
    let ws_card = CardScope::Workspace(workspace);
    write_card(source, ws_card, "one", 0).await;
    write_card(source, ws_card, "two", 1).await;

    let all = export(&of(source), &ExportScope::All, &ExportOptions::default())
        .await
        .unwrap();
    assert_eq!(
        all.format_version,
        post_cortex_storage::export_import::EXPORT_FORMAT_VERSION
    );
    assert_eq!(
        (all.metadata.message_count, all.metadata.card_count),
        (3, 4)
    );
    assert_eq!(all.sessions[0].messages, stream(source, sid).await);
    assert!(all.embeddings.iter().any(|e| e.content_id == "message:1"));

    // Into an empty store: everything comes back as it was.
    let preview = preview_of(&of(fresh), &all, &ImportOptions::default())
        .await
        .unwrap();
    assert_eq!(
        (preview.messages, preview.messages_new, preview.cards),
        (3, 3, 3)
    );
    assert!(preview.conflicts.is_empty(), "{:?}", preview.conflicts);
    let result = import(&of(fresh), all.clone(), &ImportOptions::default())
        .await
        .unwrap();
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(
        (
            result.messages_imported,
            result.cards_imported,
            result.updates_imported
        ),
        (3, 3, 2)
    );
    assert_eq!(result.message_sessions, [sid]);
    assert_eq!(stream(fresh, sid).await, stream(source, sid).await);
    assert_eq!(
        fresh.message_stats(sid).await.unwrap(),
        source.message_stats(sid).await.unwrap()
    );
    assert_eq!(
        history(fresh, CardScope::User).await,
        history(source, CardScope::User).await
    );
    assert_eq!(history(fresh, person).await, history(source, person).await);
    assert_eq!(history(fresh, person).await.len(), 1);
    assert_eq!(
        history(fresh, ws_card).await,
        history(source, ws_card).await
    );
    assert_eq!(history(fresh, ws_card).await.len(), 2);
    let vectors = fresh.get_session_vectors(&sid.to_string()).await.unwrap();
    assert!(vectors.iter().any(|(_, m)| m.id == "message:1"));
    // New turns go after the imported ones.
    let next = fresh.append_messages(sid, vec![turn("d")]).await.unwrap();
    assert_eq!(next[0].seq, 3);

    // A second import, skipping what exists, writes nothing.
    let skip = ImportOptions {
        skip_existing: true,
        ..Default::default()
    };
    let again = import(&of(fresh), all.clone(), &skip).await.unwrap();
    assert!(again.errors.is_empty(), "{:?}", again.errors);
    assert_eq!(
        (
            again.messages_imported,
            again.messages_skipped,
            again.cards_imported,
            again.cards_skipped
        ),
        (0, 3, 0, 3)
    );
    assert_eq!(fresh.message_stats(sid).await.unwrap().count, 4);
    assert_eq!(history(fresh, ws_card).await.len(), 2);
    let quiet = preview_of(&of(fresh), &all, &skip).await.unwrap();
    assert_eq!(quiet.messages_new, 0);
    assert!(quiet.conflicts.iter().all(|c| c.kind != "card"));

    // A store that has the session, other turns and its own card version.
    existing
        .save_session(
            &session,
            post_cortex_ports::storage::SessionChanges::everything(&session, &[]),
        )
        .await
        .unwrap();
    existing
        .append_messages(sid, vec![turn("x"), turn("a")])
        .await
        .unwrap();
    existing
        .save_workspace_metadata(workspace, "ws", &workspace.to_string(), "d", &[])
        .await
        .unwrap();
    write_card(existing, ws_card, "mine", 0).await;
    let preview = preview_of(&of(existing), &all, &skip).await.unwrap();
    assert_eq!((preview.messages, preview.messages_new), (3, 2));
    let card_conflict = preview.conflicts.iter().find(|c| c.kind == "card").unwrap();
    assert_eq!(
        (card_conflict.id, card_conflict.resolution),
        (workspace, ConflictResolution::Keep)
    );
    let heads = card_conflict.card.as_ref().unwrap();
    assert_eq!(
        (
            heads.target.first_line.as_str(),
            heads.archive.first_line.as_str()
        ),
        ("mine", "two")
    );
    let merged = import(&of(existing), all.clone(), &skip).await.unwrap();
    assert!(merged.errors.is_empty(), "{:?}", merged.errors);
    assert_eq!((merged.sessions_skipped, merged.workspaces_skipped), (1, 1));
    assert_eq!((merged.messages_imported, merged.messages_skipped), (2, 1));
    let turns: Vec<(u64, String)> = stream(existing, sid)
        .await
        .into_iter()
        .map(|m| (m.seq, m.turn_id))
        .collect();
    assert_eq!(
        turns,
        [
            (0, "x".into()),
            (1, "a".into()),
            (2, "b".into()),
            (3, "c".into())
        ]
    );
    let stats = existing.message_stats(sid).await.unwrap();
    assert_eq!((stats.count, stats.last_seq), (4, Some(3)));
    // b moved from 1 to 2, and its vector with it.
    let vectors = existing
        .get_session_vectors(&sid.to_string())
        .await
        .unwrap();
    let ids: Vec<&str> = vectors.iter().map(|(_, m)| m.id.as_str()).collect();
    assert_eq!(ids, ["message:2"]);
    assert_eq!(vectors[0].1.metadata["seq"], "2");
    let cards = history(existing, ws_card).await;
    let texts: Vec<&str> = cards.iter().map(|c| c.text.as_str()).collect();
    assert_eq!(texts, ["mine"]);
    assert_eq!(merged.cards_skipped, 1);
    assert_eq!(history(existing, CardScope::User).await.len(), 1);
    // And with --overwrite the archive's card history replaces it.
    let overwrite = ImportOptions {
        overwrite: true,
        ..Default::default()
    };
    write_card(existing, ws_card, "mine again", 1).await;
    import(&of(existing), all.clone(), &overwrite)
        .await
        .unwrap();
    assert_eq!(
        history(existing, ws_card).await,
        history(source, ws_card).await
    );

    // Without messages, and per scope.
    let lean = export(
        &of(source),
        &ExportScope::All,
        &ExportOptions {
            include_messages: false,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(lean.metadata.message_count, 0);
    assert!(lean.sessions.iter().all(|s| s.messages.is_empty()));
    assert!(
        !lean
            .embeddings
            .iter()
            .any(|e| e.content_type == MESSAGE_CONTENT_TYPE)
    );
    assert_eq!(lean.cards.len(), 4);
    let one = export(
        &of(source),
        &ExportScope::Workspace(workspace),
        &ExportOptions::default(),
    )
    .await
    .unwrap();
    assert!(one.cards.iter().all(|c| c.scope == ws_card));
    assert_eq!((one.cards.len(), one.metadata.message_count), (2, 3));
    let sessions = export(
        &of(source),
        &ExportScope::Sessions(vec![sid]),
        &ExportOptions::default(),
    )
    .await
    .unwrap();
    assert!(sessions.cards.is_empty());
    assert_eq!(sessions.metadata.message_count, 3);
}
