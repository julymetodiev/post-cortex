// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Watching a scope: what was written since a time first, then the scope's
//! own changes as they are stored, in full; a first-only watch ends.

use super::*;
use crate::capabilities::Memory;
use crate::capabilities::records::good_request;
use crate::capabilities::test_support::rocksdb_memory;
use futures::StreamExt;
use std::time::Duration;

async fn write(memory: &Memory, session_id: Uuid, title: &str) -> Uuid {
    let mut req = good_request();
    req.session_id = session_id;
    req.content.title = title.to_string();
    memory.records.write(req).await.unwrap().entry_id
}

async fn next(stream: &mut ActivityStream) -> ActivityEvent {
    tokio::time::timeout(Duration::from_secs(10), stream.next())
        .await
        .expect("an event in time")
        .expect("the watch still runs")
        .expect("no error")
}

#[tokio::test]
async fn a_watch_sends_what_was_written_then_its_own_changes() {
    let (memory, _dir) = rocksdb_memory().await;
    let a = memory.sessions.create(None, None).await.unwrap();
    let b = memory.sessions.create(None, None).await.unwrap();
    let first = write(&memory, a, "first").await;

    let mut stream = memory
        .detail_views
        .watch(
            Target::Session(a),
            WatchRequest {
                since: Some(DateTime::<Utc>::UNIX_EPOCH),
                first_only: false,
                changes: true,
                messages: false,
            },
        )
        .await
        .unwrap();
    let backfill = next(&mut stream).await;
    assert_eq!(
        (backfill.record_id, backfill.kind),
        (Some(first), ActivityKind::Record(RecordEventKind::Written))
    );
    assert_eq!(backfill.record.unwrap().title, "first");

    // Another session's record is not this watch's.
    write(&memory, b, "elsewhere").await;
    let second = write(&memory, a, "second").await;
    let written = next(&mut stream).await;
    assert_eq!(
        (written.record_id, written.kind),
        (Some(second), ActivityKind::Record(RecordEventKind::Written))
    );
    assert_eq!(written.session_id, Some(a));
    assert_eq!(written.record.unwrap().title, "second");

    assert!(memory.records.forget(a, second).await.unwrap());
    let forgotten = next(&mut stream).await;
    assert_eq!(
        (forgotten.record_id, forgotten.kind),
        (
            Some(second),
            ActivityKind::Record(RecordEventKind::Forgotten)
        )
    );
    assert!(forgotten.record.unwrap().validity.valid_to.is_some());

    assert!(memory.records.purge(a, first).await.unwrap());
    let removed = next(&mut stream).await;
    assert_eq!(
        (removed.record_id, removed.kind),
        (Some(first), ActivityKind::Record(RecordEventKind::Removed))
    );
    assert!(removed.record.is_none());
}

#[tokio::test]
async fn a_first_only_watch_ends_after_one_event() {
    let (memory, _dir) = rocksdb_memory().await;
    let sid = memory.sessions.create(None, None).await.unwrap();
    // Written before the call, without `since`: not news.
    write(&memory, sid, "before").await;
    let mut stream = memory
        .detail_views
        .watch(
            Target::Global,
            WatchRequest {
                first_only: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let id = write(&memory, sid, "after").await;
    assert_eq!(next(&mut stream).await.record_id, Some(id));
    assert!(
        tokio::time::timeout(Duration::from_secs(5), stream.next())
            .await
            .expect("the stream ends")
            .is_none()
    );
}

/// Asked for messages (W25), a watch sends each one appended to the
/// scope's sessions, as stored; not asked, none.
#[tokio::test]
async fn a_watch_asked_for_messages_sends_each_appended() {
    use post_cortex_core::core::message::{MessageBlock, MessageRole, NewMessage};
    use post_cortex_core::services::AppendMessagesRequest;

    let (memory, _dir) = rocksdb_memory().await;
    let sid = memory.sessions.create(None, None).await.unwrap();
    let mut stream = memory
        .detail_views
        .watch(
            Target::Session(sid),
            WatchRequest {
                messages: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let said = |turn: &str, role, author: &str, blocks: Vec<MessageBlock>| NewMessage {
        turn_id: turn.into(),
        role,
        author: Some(author.into()),
        source_session: None,
        timestamp: Utc::now(),
        text: String::new(),
        tools: Vec::new(),
        files: Vec::new(),
        parent: None,
        blocks,
    };
    memory
        .messages
        .append(AppendMessagesRequest {
            session_id: sid,
            messages: vec![
                said(
                    "p",
                    MessageRole::User,
                    "julius",
                    vec![MessageBlock::Text {
                        text: "what is left?".into(),
                    }],
                ),
                said(
                    "e",
                    MessageRole::Assistant,
                    "chief",
                    vec![MessageBlock::End {
                        stop: "end_turn".into(),
                        detail: String::new(),
                    }],
                ),
            ],
        })
        .await
        .unwrap();
    let prompt = next(&mut stream).await;
    assert_eq!(
        (prompt.kind, prompt.record_id),
        (ActivityKind::Message, None)
    );
    assert_eq!(prompt.message.unwrap().message.turn_id, "p");
    let end = next(&mut stream).await.message.unwrap().message;
    assert!(matches!(end.blocks[0], MessageBlock::End { .. }));
}
