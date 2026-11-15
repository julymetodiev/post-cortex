// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Message stream stats: many streams in one read, each with its authors,
//! kept by every write.

use chrono::{Duration, Utc};
use uuid::Uuid;

use post_cortex_core::core::message::{MessageRole, MessageStats, NewMessage};

use crate::storage::Stores;

/// A user turn `turn_id` by `author`, `age_days` old.
fn aged(turn_id: &str, author: Option<&str>, age_days: i64) -> NewMessage {
    NewMessage {
        turn_id: turn_id.to_string(),
        role: MessageRole::User,
        author: author.map(str::to_string),
        source_session: None,
        timestamp: Utc::now() - Duration::days(age_days),
        text: format!("turn {turn_id}"),
        tools: Vec::new(),
        files: Vec::new(),
        parent: None,
        blocks: Vec::new(),
    }
}

/// A user turn `turn_id` by `author`, now.
fn turn(turn_id: &str, author: Option<&str>) -> NewMessage {
    aged(turn_id, author, 0)
}

/// The stats of many streams come in one read, in the order asked, with
/// each stream's authors; every write keeps them: appends, a deleted
/// message, retention and the stream's deletion.
pub async fn stats_of_many_streams_keep_their_authors(stores: &Stores) {
    let (s1, empty, s3) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let m = &stores.messages;
    m.append_messages(
        s1,
        vec![
            turn("a", Some("dev")),
            turn("b", None),
            turn("c", Some("claude")),
        ],
    )
    .await
    .unwrap();
    m.append_messages(s1, vec![turn("d", Some("dev"))])
        .await
        .unwrap();
    m.append_messages(s3, vec![turn("x", Some("hermes"))])
        .await
        .unwrap();

    let all = m.message_stats_of(&[s3, empty, s1]).await.unwrap();
    let one_by_one = [
        m.message_stats(s3).await.unwrap(),
        m.message_stats(empty).await.unwrap(),
        m.message_stats(s1).await.unwrap(),
    ];
    assert_eq!(
        all, one_by_one,
        "the same as one stream at a time, in order"
    );
    assert_eq!(all[1], MessageStats::default());
    assert_eq!(all[2].count, 4);
    assert_eq!(all[2].authors, ["claude", "dev"], "each once, sorted");
    assert_eq!(all[0].authors, ["hermes"]);
    assert!(m.message_stats_of(&[]).await.unwrap().is_empty());

    // The only "claude" turn goes: so does its author.
    assert!(m.delete_message(s1, 2).await.unwrap());
    let after = m.message_stats(s1).await.unwrap();
    assert_eq!(
        (after.count, after.authors.clone()),
        (3, vec!["dev".to_string()])
    );
    assert_eq!(m.message_stats_of(&[s1]).await.unwrap(), [after]);

    // Retention takes s3's only (old) turn.
    m.append_messages(s3, vec![aged("y", Some("old"), 120)])
        .await
        .unwrap();
    assert_eq!(
        m.message_stats(s3).await.unwrap().authors,
        ["hermes", "old"]
    );
    m.delete_messages_before(Utc::now() - Duration::days(30))
        .await
        .unwrap();
    let s3_after = m.message_stats(s3).await.unwrap();
    assert_eq!(
        (s3_after.count, s3_after.authors.clone()),
        (1, vec!["hermes".to_string()])
    );
    assert_eq!(m.message_stats_of(&[s3]).await.unwrap(), [s3_after]);

    // The stream's deletion empties its stats; a new turn starts them again.
    m.delete_session_messages(s1).await.unwrap();
    assert_eq!(
        m.message_stats_of(&[s1]).await.unwrap(),
        [MessageStats::default()]
    );
    m.append_messages(s1, vec![turn("e", Some("chief"))])
        .await
        .unwrap();
    let again = m.message_stats(s1).await.unwrap();
    assert_eq!(
        (again.count, again.first_seq, again.authors.clone()),
        (1, Some(4), vec!["chief".to_string()])
    );
}
