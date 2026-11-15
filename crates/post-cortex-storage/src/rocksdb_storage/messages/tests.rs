// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;
use post_cortex_core::core::message::MessageRole;

fn turn(turn_id: &str, age_days: i64) -> NewMessage {
    NewMessage {
        turn_id: turn_id.to_string(),
        role: MessageRole::User,
        author: None,
        source_session: None,
        timestamp: Utc::now() - chrono::Duration::days(age_days),
        text: "turn".to_string(),
        tools: vec![],
        files: vec![],
        parent: None,
        blocks: Vec::new(),
    }
}

/// A stream written before `message_count` existed is counted once,
/// and the count is kept from then on.
#[tokio::test]
async fn a_stream_without_a_count_is_counted_and_then_kept() {
    let dir = tempfile::tempdir().unwrap();
    let storage = RealRocksDBStorage::new(dir.path()).await.unwrap();
    let session = Uuid::new_v4();
    storage
        .append_messages(session, vec![turn("a", 120), turn("b", 0), turn("c", 0)])
        .await
        .unwrap();
    let count_key = message_count_key(session);
    storage.db.delete(count_key.as_bytes()).unwrap();

    assert_eq!(storage.message_stats(session).await.unwrap().count, 3);
    assert!(storage.db.get(count_key.as_bytes()).unwrap().is_some());

    storage.db.delete(count_key.as_bytes()).unwrap();
    storage
        .delete_messages_before(Utc::now() - chrono::Duration::days(90))
        .await
        .unwrap();
    assert_eq!(storage.message_stats(session).await.unwrap().count, 2);

    storage.db.delete(count_key.as_bytes()).unwrap();
    storage
        .append_messages(session, vec![turn("d", 0)])
        .await
        .unwrap();
    assert_eq!(storage.message_stats(session).await.unwrap().count, 3);
}

/// A stream written before `message_authors` existed gets its authors
/// from its rows once, kept from then on; an append to it adds to them.
#[tokio::test]
async fn a_stream_without_authors_gets_them_from_its_rows_and_then_keeps_them() {
    let dir = tempfile::tempdir().unwrap();
    let storage = RealRocksDBStorage::new(dir.path()).await.unwrap();
    let session = Uuid::new_v4();
    let by = |id: &str, author: Option<&str>| NewMessage {
        author: author.map(str::to_string),
        ..turn(id, 0)
    };
    storage
        .append_messages(
            session,
            vec![by("a", Some("dev")), by("b", None), by("c", Some("claude"))],
        )
        .await
        .unwrap();
    let authors_key = message_authors_key(session);
    storage.db.delete(authors_key.as_bytes()).unwrap();

    let stats = storage.message_stats_of(&[session]).await.unwrap();
    assert_eq!(stats[0].authors, ["claude", "dev"]);
    assert!(storage.db.get(authors_key.as_bytes()).unwrap().is_some());

    storage.db.delete(authors_key.as_bytes()).unwrap();
    storage
        .append_messages(session, vec![by("d", Some("hermes"))])
        .await
        .unwrap();
    assert_eq!(
        storage.message_stats(session).await.unwrap().authors,
        ["claude", "dev", "hermes"]
    );
}
