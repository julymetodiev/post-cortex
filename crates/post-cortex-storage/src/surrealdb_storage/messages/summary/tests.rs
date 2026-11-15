// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use chrono::Utc;
use post_cortex_core::core::message::{MessageRole, NewMessage};
use post_cortex_ports::storage::MessageStore;

use super::*;
use crate::surrealdb_storage::SurrealDBStorage;

fn by(turn_id: &str, author: Option<&str>) -> NewMessage {
    NewMessage {
        turn_id: turn_id.to_string(),
        role: MessageRole::User,
        author: author.map(str::to_string),
        source_session: None,
        timestamp: Utc::now(),
        text: "turn".to_string(),
        tools: vec![],
        files: vec![],
        parent: None,
        blocks: Vec::new(),
    }
}

/// How many `message_stream` rows `session` has.
async fn rows_of(storage: &SurrealDBStorage, session: Uuid) -> usize {
    let mut response = storage
        .db()
        .query("SELECT * FROM message_stream WHERE id = type::record('message_stream', $sid);")
        .bind(("sid", session.to_string()))
        .await
        .unwrap()
        .check()
        .unwrap();
    let rows: Vec<surrealdb::types::Value> = response.take(0).unwrap();
    rows.len()
}

/// A stream written before summaries existed gets one from its rows the
/// first time it is read, kept from then on; a session never written gets
/// none; an append to a stream without one recounts it from its rows.
#[tokio::test]
async fn a_stream_without_a_summary_gets_one_from_its_rows() {
    let storage = SurrealDBStorage::new("mem://", None, None, None, None)
        .await
        .unwrap();
    let (session, never) = (Uuid::new_v4(), Uuid::new_v4());
    storage
        .append_messages(
            session,
            vec![by("a", Some("dev")), by("b", None), by("c", Some("claude"))],
        )
        .await
        .unwrap();
    storage
        .db()
        .query("DELETE message_stream;")
        .await
        .unwrap()
        .check()
        .unwrap();
    assert_eq!(rows_of(&storage, session).await, 0);

    let stats = storage.message_stats_of(&[session, never]).await.unwrap();
    assert_eq!(
        (stats[0].count, stats[0].first_seq, stats[0].last_seq),
        (3, Some(0), Some(2))
    );
    assert_eq!(stats[0].authors, ["claude", "dev"]);
    assert_eq!(stats[1], MessageStats::default());
    assert_eq!(rows_of(&storage, session).await, 1, "kept");
    assert_eq!(
        rows_of(&storage, never).await,
        0,
        "none for a stream never written"
    );

    storage
        .db()
        .query("DELETE message_stream;")
        .await
        .unwrap()
        .check()
        .unwrap();
    storage
        .append_messages(session, vec![by("d", Some("hermes"))])
        .await
        .unwrap();
    let after = storage.message_stats(session).await.unwrap();
    assert_eq!((after.count, after.last_seq), (4, Some(3)));
    assert_eq!(after.authors, ["claude", "dev", "hermes"]);
}

/// Two reads that find the same streams without a summary make them
/// together: both answer, with the same stats.
#[tokio::test]
async fn reads_making_the_same_summaries_together_both_answer() {
    let storage = SurrealDBStorage::new("mem://", None, None, None, None)
        .await
        .unwrap();
    let sessions: Vec<Uuid> = (0..6).map(|_| Uuid::new_v4()).collect();
    for (i, session) in sessions.iter().enumerate() {
        storage
            .append_messages(*session, vec![by(&format!("t{i}"), Some("dev"))])
            .await
            .unwrap();
    }
    storage
        .db()
        .query("DELETE message_stream;")
        .await
        .unwrap()
        .check()
        .unwrap();

    let (a, b) = tokio::join!(
        storage.message_stats_of(&sessions),
        storage.message_stats_of(&sessions)
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_eq!(a, b);
    assert!(a.iter().all(|s| s.count == 1 && s.authors == ["dev"]));
}
