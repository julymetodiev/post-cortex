// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Imports into a target that has data: diverged cards and chunked message
//! vectors.

use crate::capabilities::transfer::tests::Backend;
use crate::capabilities::transfer::tests::{export, import, of, preview_of};
use crate::capabilities::transfer::*;
use chrono::Utc;
use post_cortex_core::core::card::{Card, CardScope};
use post_cortex_core::session::active_session::ActiveSession;
use post_cortex_ports::storage::{CardStore, CardWrite, VectorStore};
use post_cortex_ports::{MESSAGE_CONTENT_TYPE, VectorMetadata};
use uuid::Uuid;

pub async fn write_card(storage: &impl CardStore, scope: CardScope, text: &str, expected: u64) {
    let written = storage
        .put_card(
            scope,
            text.to_string(),
            Some(format!("by {text}")),
            expected,
        )
        .await
        .unwrap();
    assert!(matches!(written, CardWrite::Written(_)), "{written:?}");
}

pub async fn history(storage: &impl CardStore, scope: CardScope) -> Vec<Card> {
    let mut cards = storage.card_history(scope, 100).await.unwrap();
    cards.reverse();
    cards
}

/// `version` of the card of `scope` with `text`, written at `at` (RFC 3339).
pub fn card_at(scope: CardScope, version: u64, text: &str, at: &str) -> Card {
    Card {
        scope,
        version,
        text: text.to_string(),
        updated_at: chrono::DateTime::parse_from_rfc3339(at)
            .unwrap()
            .with_timezone(&Utc),
        author: None,
    }
}

/// Diverged card histories are decided by the time of their latest
/// versions, never by version numbers, and only when asked. The observed
/// case: the target's v1 (2026-10-21 08:01, current) against an archive
/// whose v2 is a day older (2026-10-20 14:42): `skip_existing` keeps the
/// target and reports both heads, so does `cards_newer`; an archive head
/// written later is taken only with `cards_newer`, as the next version;
/// `overwrite` replaces the history; one history continuing the other is
/// fast-forwarded as before.
pub async fn diverged_cards_keep_the_target_unless_asked(target: &impl Backend) {
    let workspace = Uuid::new_v4();
    target
        .save_workspace_metadata(workspace, "ws", &workspace.to_string(), "d", &[])
        .await
        .unwrap();
    let scope = CardScope::Workspace(workspace);
    let here = card_at(scope, 1, "staging, current\nmore", "2026-10-21T08:01:00Z");
    target
        .restore_card_versions(scope, vec![here.clone()], true)
        .await
        .unwrap();
    let source = export(
        &of(target),
        &ExportScope::Workspace(workspace),
        &ExportOptions::default(),
    )
    .await
    .unwrap();
    let archive = |cards: Vec<Card>| {
        let mut data = source.clone();
        data.cards = cards;
        data.update_counts();
        data
    };
    let older = archive(vec![
        card_at(scope, 1, "production v1", "2026-10-19T09:00:00Z"),
        card_at(scope, 2, "production, older", "2026-10-20T14:42:00Z"),
    ]);
    let skip = ImportOptions {
        skip_existing: true,
        ..Default::default()
    };
    let newer = ImportOptions {
        cards_newer: true,
        ..skip.clone()
    };
    for options in [&skip, &newer] {
        let preview = preview_of(&of(target), &older, options).await.unwrap();
        let conflict = preview.conflicts.iter().find(|c| c.kind == "card").unwrap();
        assert_eq!(conflict.resolution, ConflictResolution::Keep);
        let heads = conflict.card.as_ref().unwrap();
        assert_eq!(heads.target.updated_at, here.updated_at);
        assert_eq!(
            (heads.archive.version, heads.archive.first_line.as_str()),
            (2, "production, older")
        );
        let result = import(&of(target), older.clone(), options).await.unwrap();
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        assert_eq!((result.cards_imported, result.cards_skipped), (0, 1));
        assert_eq!(history(target, scope).await, std::slice::from_ref(&here));
    }

    // The archive's head written after the target's: only when asked.
    let later = archive(vec![
        card_at(scope, 1, "production v1", "2026-10-19T09:00:00Z"),
        card_at(scope, 2, "production, later", "2026-10-21T09:30:00Z"),
    ]);
    import(&of(target), later.clone(), &skip).await.unwrap();
    assert_eq!(history(target, scope).await, std::slice::from_ref(&here));
    let preview = preview_of(&of(target), &later, &newer).await.unwrap();
    assert!(
        preview
            .conflicts
            .iter()
            .any(|c| c.kind == "card" && c.resolution == ConflictResolution::Update)
    );
    let result = import(&of(target), later.clone(), &newer).await.unwrap();
    assert_eq!(result.cards_imported, 1);
    let texts: Vec<(u64, String)> = history(target, scope)
        .await
        .into_iter()
        .map(|c| (c.version, c.text))
        .collect();
    assert_eq!(
        texts,
        [
            (1, "staging, current\nmore".to_string()),
            (2, "production, later".to_string())
        ]
    );
    // The same latest text now: a re-import is quiet.
    let quiet = preview_of(&of(target), &later, &newer).await.unwrap();
    assert!(quiet.conflicts.iter().all(|c| c.kind != "card"));

    // --overwrite replaces the history, even with an older archive.
    let overwrite = ImportOptions {
        overwrite: true,
        ..Default::default()
    };
    import(&of(target), older.clone(), &overwrite)
        .await
        .unwrap();
    assert_eq!(history(target, scope).await, older.cards);

    // One history continuing the other: fast-forward, whatever the times.
    let mut ahead = older.cards.clone();
    ahead.push(card_at(scope, 3, "production v3", "2026-10-01T00:00:00Z"));
    let ahead = archive(ahead);
    let result = import(&of(target), ahead.clone(), &skip).await.unwrap();
    assert_eq!(result.cards_imported, 1);
    assert_eq!(history(target, scope).await, ahead.cards);
    // And an archive behind the target leaves it.
    let result = import(&of(target), older.clone(), &skip).await.unwrap();
    assert_eq!(result.cards_skipped, 1);
    assert_eq!(history(target, scope).await, ahead.cards);
}

/// A vector of message `seq` as the vectorizer writes it: chunk `n` (1 is
/// the message's own id) with `seq` and, for later chunks, `parent`.
pub fn message_chunk(session: Uuid, seq: u64, n: usize) -> (Vec<f32>, VectorMetadata) {
    let mut vector = vec![0.0; 256];
    vector[(seq as usize * 7 + n) % 256] = 1.0;
    let parent = format!("message:{seq}");
    let id = match n {
        1 => parent.clone(),
        n => format!("{parent}#{n}"),
    };
    let mut metadata = VectorMetadata::new(
        id,
        format!("turn {seq} chunk {n}"),
        session.to_string(),
        MESSAGE_CONTENT_TYPE.to_string(),
    );
    metadata.metadata.insert("seq".to_string(), seq.to_string());
    metadata
        .metadata
        .insert("role".to_string(), "assistant".to_string());
    if n > 1 {
        metadata.metadata.insert("parent".to_string(), parent);
    }
    (vector, metadata)
}

/// The message vectors of `session` in `storage`, by content id:
/// `(content id, text, seq, parent, vector)`.
pub async fn message_chunks(
    storage: &impl VectorStore,
    session: Uuid,
) -> Vec<(String, String, String, Option<String>, Vec<f32>)> {
    let mut found: Vec<_> = storage
        .get_session_vectors(&session.to_string())
        .await
        .unwrap()
        .into_iter()
        .filter(|(_, m)| m.content_type == MESSAGE_CONTENT_TYPE)
        .map(|(v, m)| {
            (
                m.id.clone(),
                m.text.to_string(),
                m.metadata["seq"].clone(),
                m.metadata.get("parent").cloned(),
                v,
            )
        })
        .collect();
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found
}

/// Every chunk vector of a long message travels with it: into an empty
/// store with the same ids, texts, metadata and vectors, and into a store
/// whose stream shifts the message to another position with ids and
/// metadata renamed to that position. Vectors of positions no message has
/// are not exported.
pub async fn chunked_message_vectors_round_trip(
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
            post_cortex_ports::storage::SessionChanges::everything(&session, &[]),
        )
        .await
        .unwrap();
    source
        .append_messages(sid, vec![turn("a"), turn("b"), turn("c")])
        .await
        .unwrap();
    // b (seq 1) is long: three chunks; c (seq 2) one; seq 9 has no message.
    for (seq, chunks) in [(1, 3), (2, 1), (9, 2)] {
        for n in 1..=chunks {
            let (vector, metadata) = message_chunk(sid, seq, n);
            source
                .upsert_vectors(vec![(vector, metadata)])
                .await
                .unwrap();
        }
    }
    let all = export(&of(source), &ExportScope::All, &ExportOptions::default())
        .await
        .unwrap();
    let mut exported: Vec<&str> = all
        .embeddings
        .iter()
        .filter(|e| e.content_type == MESSAGE_CONTENT_TYPE)
        .map(|e| e.content_id.as_str())
        .collect();
    exported.sort_unstable();
    assert_eq!(
        exported,
        ["message:1", "message:1#2", "message:1#3", "message:2"]
    );

    let result = import(&of(fresh), all.clone(), &ImportOptions::default())
        .await
        .unwrap();
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(result.messages_imported, 3);
    let mut kept = message_chunks(source, sid).await;
    kept.retain(|c| c.2 != "9");
    assert_eq!(message_chunks(fresh, sid).await, kept);

    // b and c land one position later.
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
    let skip = ImportOptions {
        skip_existing: true,
        ..Default::default()
    };
    let merged = import(&of(existing), all, &skip).await.unwrap();
    assert!(merged.errors.is_empty(), "{:?}", merged.errors);
    assert_eq!((merged.messages_imported, merged.messages_skipped), (2, 1));
    let moved: Vec<(String, String, String, Option<String>)> = message_chunks(existing, sid)
        .await
        .into_iter()
        .map(|(id, text, seq, parent, _)| (id, text, seq, parent))
        .collect();
    let chunk = |id: &str, text: &str, seq: &str, parent: Option<&str>| {
        (
            id.to_string(),
            text.to_string(),
            seq.to_string(),
            parent.map(str::to_string),
        )
    };
    assert_eq!(
        moved,
        [
            chunk("message:2", "turn 1 chunk 1", "2", None),
            chunk("message:2#2", "turn 1 chunk 2", "2", Some("message:2")),
            chunk("message:2#3", "turn 1 chunk 3", "2", Some("message:2")),
            chunk("message:3", "turn 2 chunk 1", "3", None),
        ]
    );
}

pub fn turn(turn_id: &str) -> post_cortex_core::core::message::NewMessage {
    post_cortex_core::core::message::NewMessage {
        turn_id: turn_id.to_string(),
        role: post_cortex_core::core::message::MessageRole::User,
        author: None,
        source_session: None,
        timestamp: Utc::now(),
        text: format!("turn {turn_id}"),
        tools: vec![],
        files: vec![],
        parent: None,
        blocks: Vec::new(),
    }
}
