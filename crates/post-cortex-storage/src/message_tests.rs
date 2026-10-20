// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! [`MessageStorage`] behaviour, run against every backend.

use chrono::{Duration, Utc};
use uuid::Uuid;

use crate::traits::{MessageStorage, Storage};
use post_cortex_core::core::message::{MessageRole, NewMessage};
use post_cortex_core::session::active_session::ActiveSession;

fn turn(turn_id: &str, role: MessageRole, text: &str, age_days: i64) -> NewMessage {
    NewMessage {
        turn_id: turn_id.to_string(),
        role,
        author: (role == MessageRole::Assistant).then(|| "claude".to_string()),
        source_session: Some("cc-1".to_string()),
        timestamp: Utc::now() - Duration::days(age_days),
        text: text.to_string(),
        tools: vec!["Edit".to_string()],
        files: vec!["src/main.rs".to_string()],
    }
}

async fn append_assigns_positions_and_skips_known_turns(storage: &impl MessageStorage) {
    let session = Uuid::new_v4();
    let first = storage
        .append_messages(
            session,
            vec![
                turn("t1", MessageRole::User, "how do we store messages?", 0),
                turn("t2", MessageRole::Assistant, "in their own stream", 0),
                // Repeated within the batch.
                turn("t1", MessageRole::User, "how do we store messages?", 0),
            ],
        )
        .await
        .unwrap();
    assert_eq!(
        first
            .iter()
            .map(|m| (m.seq, m.turn_id.as_str()))
            .collect::<Vec<_>>(),
        vec![(0, "t1"), (1, "t2")]
    );

    // A retried batch: t2 is stored already, t3 is new.
    let second = storage
        .append_messages(
            session,
            vec![
                turn("t2", MessageRole::Assistant, "in their own stream", 0),
                turn("t3", MessageRole::User, "and retention?", 0),
            ],
        )
        .await
        .unwrap();
    assert_eq!(
        second
            .iter()
            .map(|m| (m.seq, m.turn_id.as_str()))
            .collect::<Vec<_>>(),
        vec![(2, "t3")]
    );

    let stored = storage.load_messages(session, 0..10).await.unwrap();
    assert_eq!(stored.len(), 3);
    assert_eq!(stored[1], first[1], "round trip keeps every field");
    assert_eq!(stored[1].author.as_deref(), Some("claude"));
    assert_eq!(stored[0].author, None);
    assert_eq!(
        storage
            .load_messages(session, 1..2)
            .await
            .unwrap()
            .iter()
            .map(|m| m.seq)
            .collect::<Vec<_>>(),
        vec![1]
    );

    // Streams are per session.
    let other = storage
        .append_messages(
            Uuid::new_v4(),
            vec![turn("t1", MessageRole::User, "same turn id elsewhere", 0)],
        )
        .await
        .unwrap();
    assert_eq!(other[0].seq, 0);
}

async fn retention_deletes_old_messages_and_keeps_positions(storage: &impl MessageStorage) {
    let session = Uuid::new_v4();
    storage
        .append_messages(
            session,
            vec![
                turn("old1", MessageRole::User, "old question", 120),
                turn("old2", MessageRole::Assistant, "old answer", 100),
                turn("new1", MessageRole::User, "new question", 1),
            ],
        )
        .await
        .unwrap();

    let mut deleted = storage
        .delete_messages_before(Utc::now() - Duration::days(90))
        .await
        .unwrap();
    deleted.retain(|(s, _)| *s == session);
    deleted.sort();
    assert_eq!(deleted, vec![(session, 0), (session, 1)]);

    let left = storage.load_messages(session, 0..10).await.unwrap();
    assert_eq!(left.iter().map(|m| m.seq).collect::<Vec<_>>(), vec![2]);

    // Positions are not reused once the oldest are gone.
    let more = storage
        .append_messages(
            session,
            vec![turn("new2", MessageRole::Assistant, "more", 0)],
        )
        .await
        .unwrap();
    assert_eq!(more[0].seq, 3);
}

async fn deleting_the_session_deletes_its_stream(storage: &impl Storage) {
    let session = ActiveSession::new(Uuid::new_v4(), Some("s".to_string()), None);
    let id = session.id();
    storage.save_session(&session).await.unwrap();
    storage
        .append_messages(id, vec![turn("t1", MessageRole::User, "hello", 0)])
        .await
        .unwrap();

    storage.delete_session(id).await.unwrap();
    assert!(storage.load_messages(id, 0..10).await.unwrap().is_empty());
    // The stream starts over, and the old turn id is free again.
    let again = storage
        .append_messages(id, vec![turn("t1", MessageRole::User, "hello", 0)])
        .await
        .unwrap();
    assert_eq!(again[0].seq, 0);
}

async fn exercise(storage: &impl Storage) {
    append_assigns_positions_and_skips_known_turns(storage).await;
    retention_deletes_old_messages_and_keeps_positions(storage).await;
    deleting_the_session_deletes_its_stream(storage).await;
}

#[tokio::test]
async fn rocksdb_message_stream() {
    let dir = tempfile::tempdir().unwrap();
    let storage = crate::RealRocksDBStorage::new(dir.path()).await.unwrap();
    exercise(&storage).await;
}

#[cfg(feature = "surrealdb-storage")]
#[tokio::test]
async fn surrealdb_message_stream() {
    let storage = crate::SurrealDBStorage::new("mem://", None, None, None, None)
        .await
        .unwrap();
    exercise(&storage).await;
}
