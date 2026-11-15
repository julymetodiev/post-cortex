// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Messages: positions, idempotent appends, retention, pages, deletion,
//! and the stream going with its session.

use chrono::{Duration, Utc};
use uuid::Uuid;

use post_cortex_core::core::message::{
    MessageCursor, MessageOrder, MessageRole, MessageStats, NewMessage,
};
use post_cortex_core::session::active_session::ActiveSession;

use crate::storage::Stores;

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
        parent: None,
        blocks: Vec::new(),
    }
}

/// A user turn `turn_id`, now.
fn one(turn_id: &str) -> NewMessage {
    turn(turn_id, MessageRole::User, &format!("turn {turn_id}"), 0)
}

/// Appends take the next positions and skip turns stored already; streams
/// are per session.
pub async fn append_assigns_positions_and_skips_known_turns(stores: &Stores) {
    let session = Uuid::new_v4();
    let first = stores
        .messages
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
    let second = stores
        .messages
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

    let stored = stores.messages.load_messages(session, 0..10).await.unwrap();
    assert_eq!(stored.len(), 3);
    assert_eq!(stored[1], first[1], "round trip keeps every field");
    assert_eq!(stored[1].author.as_deref(), Some("claude"));
    assert_eq!(stored[0].author, None);
    assert_eq!(
        stores
            .messages
            .load_messages(session, 1..2)
            .await
            .unwrap()
            .iter()
            .map(|m| m.seq)
            .collect::<Vec<_>>(),
        vec![1]
    );

    // Streams are per session.
    let other = stores
        .messages
        .append_messages(
            Uuid::new_v4(),
            vec![turn("t1", MessageRole::User, "same turn id elsewhere", 0)],
        )
        .await
        .unwrap();
    assert_eq!(other[0].seq, 0);
}

/// Retention deletes old messages; their positions are not reused.
pub async fn retention_deletes_old_messages_and_keeps_positions(stores: &Stores) {
    let session = Uuid::new_v4();
    stores
        .messages
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

    let mut deleted = stores
        .messages
        .delete_messages_before(Utc::now() - Duration::days(90))
        .await
        .unwrap();
    deleted.retain(|(s, _)| *s == session);
    deleted.sort();
    assert_eq!(deleted, vec![(session, 0), (session, 1)]);

    let left = stores.messages.load_messages(session, 0..10).await.unwrap();
    assert_eq!(left.iter().map(|m| m.seq).collect::<Vec<_>>(), vec![2]);

    // Positions are not reused once the oldest are gone.
    let more = stores
        .messages
        .append_messages(
            session,
            vec![turn("new2", MessageRole::Assistant, "more", 0)],
        )
        .await
        .unwrap();
    assert_eq!(more[0].seq, 3);
}

/// Deleting the session deletes its stream, which then starts over.
pub async fn deleting_the_session_deletes_its_stream(stores: &Stores) {
    let session = ActiveSession::new(
        Uuid::new_v4(),
        Some("s".to_string()),
        None,
        chrono::Utc::now(),
    );
    let id = session.id();
    stores
        .sessions
        .save_session(
            &session,
            crate::storage::SessionChanges::everything(&session, &[]),
        )
        .await
        .unwrap();
    stores
        .messages
        .append_messages(id, vec![turn("t1", MessageRole::User, "hello", 0)])
        .await
        .unwrap();

    stores.sessions.delete_session(id).await.unwrap();
    assert!(
        stores
            .messages
            .load_messages(id, 0..10)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        stores.messages.message_stats(id).await.unwrap(),
        MessageStats::default()
    );
    // The stream starts over, and the old turn id is free again.
    let again = stores
        .messages
        .append_messages(id, vec![turn("t1", MessageRole::User, "hello", 0)])
        .await
        .unwrap();
    assert_eq!(again[0].seq, 0);
    assert_eq!(stores.messages.message_stats(id).await.unwrap().count, 1);
}

/// Stats and pages read the stream in both orders and start after what
/// retention took.
pub async fn stats_and_pages_read_the_stream_and_skip_expired_heads(stores: &Stores) {
    use MessageCursor::{Before, From};
    use MessageOrder::{Asc, Desc};

    let session = Uuid::new_v4();
    assert_eq!(
        stores.messages.message_stats(session).await.unwrap(),
        MessageStats::default()
    );
    assert!(seqs(stores, session, From(0), 10, Asc).await.is_empty());

    // seq 0 and 1 are past retention.
    let turns = (0..6)
        .map(|i| {
            let age = if i < 2 { 120 } else { 0 };
            turn(&format!("p{i}"), MessageRole::User, "turn", age)
        })
        .collect();
    stores
        .messages
        .append_messages(session, turns)
        .await
        .unwrap();
    let stored = stores
        .messages
        .load_messages(session, 0..u64::MAX)
        .await
        .unwrap();

    let stats = stores.messages.message_stats(session).await.unwrap();
    assert_eq!(
        stats,
        MessageStats {
            count: 6,
            first_seq: Some(0),
            last_seq: Some(5),
            first_at: Some(stored[0].timestamp),
            last_at: Some(stored[5].timestamp),
            authors: Vec::new(),
        }
    );

    assert_eq!(seqs(stores, session, From(0), 2, Asc).await, [0, 1]);
    assert_eq!(seqs(stores, session, From(2), 2, Asc).await, [2, 3]);
    assert_eq!(seqs(stores, session, From(5), 2, Asc).await, [5]);
    assert!(seqs(stores, session, From(6), 2, Asc).await.is_empty());
    assert_eq!(
        seqs(stores, session, Before(u64::MAX), 2, Desc).await,
        [5, 4]
    );
    assert_eq!(
        seqs(stores, session, Before(4), 10, Desc).await,
        [3, 2, 1, 0]
    );
    assert_eq!(seqs(stores, session, Before(3), 2, Asc).await, [0, 1]);
    assert_eq!(seqs(stores, session, From(3), 2, Desc).await, [5, 4]);
    assert!(seqs(stores, session, From(0), 0, Asc).await.is_empty());
    let page = stores
        .messages
        .load_messages_page(session, From(4), 1, Asc)
        .await
        .unwrap();
    assert_eq!(page, stored[4..5], "a page holds whole messages");

    // Other streams are not read.
    let other = Uuid::new_v4();
    stores
        .messages
        .append_messages(other, vec![turn("p0", MessageRole::User, "x", 0)])
        .await
        .unwrap();
    assert_eq!(
        stores.messages.message_stats(session).await.unwrap().count,
        6
    );
    assert_eq!(seqs(stores, other, Before(u64::MAX), 10, Desc).await, [0]);

    // Retention takes the head: stats and pages start after it.
    let mut deleted = stores
        .messages
        .delete_messages_before(Utc::now() - Duration::days(90))
        .await
        .unwrap();
    deleted.retain(|(s, _)| *s == session);
    assert_eq!(deleted.len(), 2);
    let stats = stores.messages.message_stats(session).await.unwrap();
    assert_eq!(
        (stats.count, stats.first_seq, stats.last_seq),
        (4, Some(2), Some(5))
    );
    assert_eq!(stats.first_at, Some(stored[2].timestamp));
    assert_eq!(seqs(stores, session, From(0), 2, Asc).await, [2, 3]);
    assert_eq!(seqs(stores, session, Before(4), 10, Desc).await, [3, 2]);
    assert!(seqs(stores, session, Before(2), 10, Desc).await.is_empty());

    stores
        .messages
        .append_messages(session, vec![turn("p6", MessageRole::User, "x", 0)])
        .await
        .unwrap();
    let stats = stores.messages.message_stats(session).await.unwrap();
    assert_eq!(
        (stats.count, stats.first_seq, stats.last_seq),
        (5, Some(2), Some(6))
    );
}

async fn seqs(
    stores: &Stores,
    session: Uuid,
    cursor: MessageCursor,
    limit: usize,
    order: MessageOrder,
) -> Vec<u64> {
    stores
        .messages
        .load_messages_page(session, cursor, limit, order)
        .await
        .unwrap()
        .iter()
        .map(|m| m.seq)
        .collect()
}

/// One turn or a whole stream is deleted, the stats follow, and positions
/// are never reused.
pub async fn messages_are_deleted_without_reusing_positions(stores: &Stores) {
    let session = Uuid::new_v4();
    stores
        .messages
        .append_messages(session, vec![one("a"), one("b"), one("c")])
        .await
        .unwrap();
    assert!(stores.messages.delete_message(session, 1).await.unwrap());
    assert!(!stores.messages.delete_message(session, 1).await.unwrap());
    assert!(!stores.messages.delete_message(session, 99).await.unwrap());
    let stats = stores.messages.message_stats(session).await.unwrap();
    assert_eq!(
        (stats.count, stats.first_seq, stats.last_seq),
        (2, Some(0), Some(2))
    );
    let left = stores.messages.load_messages(session, 0..10).await.unwrap();
    assert_eq!(left.iter().map(|m| m.seq).collect::<Vec<_>>(), [0, 2]);

    let mut deleted = stores
        .messages
        .delete_session_messages(session)
        .await
        .unwrap();
    deleted.sort_unstable();
    assert_eq!(deleted, [0, 2]);
    assert_eq!(
        stores.messages.message_stats(session).await.unwrap().count,
        0
    );
    assert!(
        stores
            .messages
            .delete_session_messages(session)
            .await
            .unwrap()
            .is_empty()
    );

    // A deleted turn can be captured again, at a new position.
    let again = stores
        .messages
        .append_messages(session, vec![one("b"), one("d")])
        .await
        .unwrap();
    assert_eq!(again.iter().map(|m| m.seq).collect::<Vec<_>>(), [3, 4]);
    assert_eq!(
        stores.messages.message_stats(session).await.unwrap().count,
        2
    );
}

/// An agent's turn reads back whole: the turn it follows and its blocks
/// (a tool call's input any JSON), next to a captured turn without either.
pub async fn an_agents_turn_keeps_its_parent_and_blocks(stores: &Stores) {
    use post_cortex_core::core::message::MessageBlock;

    let session = Uuid::new_v4();
    let blocks = vec![
        MessageBlock::Thinking {
            text: "read it first".to_string(),
            signature: None,
        },
        MessageBlock::ToolUse {
            id: "c1".to_string(),
            name: "fs_read".to_string(),
            input: serde_json::json!({ "path": "a.rs", "lines": [1, 2], "deep": { "x": null } }),
        },
    ];
    let mut call = turn("t2", MessageRole::Assistant, "", 0);
    call.parent = Some("t1".to_string());
    call.blocks = blocks.clone();
    stores
        .messages
        .append_messages(session, vec![one("t1"), call])
        .await
        .unwrap();
    let page = stores
        .messages
        .load_messages_page(session, MessageCursor::From(0), 10, MessageOrder::Asc)
        .await
        .unwrap();
    assert_eq!(page.len(), 2);
    assert_eq!((page[0].parent.as_deref(), page[0].blocks.len()), (None, 0));
    assert_eq!(page[1].parent.as_deref(), Some("t1"));
    assert_eq!(page[1].blocks, blocks);
}
