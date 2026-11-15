// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;
use crate::memory_system::backends_tests::TestSystem;
use crate::memory_system::{MemoryRuntime, SystemConfig};
use chrono::Duration;
use post_cortex_core::core::message::MessageRole;
use post_cortex_ports::{MESSAGE_CONTENT_TYPE, VectorMetadata};

async fn system() -> (TestSystem, tempfile::TempDir) {
    let dir = tempfile::Builder::new()
        .prefix("pcx_messages_")
        .tempdir()
        .unwrap();
    let config = SystemConfig {
        enable_embeddings: false,
        ..Default::default()
    };
    (TestSystem::new(crate::memory_system::backends_tests::rocksdb_runtime(&dir, config).await.unwrap()), dir)
}

fn turn(turn_id: &str, age_days: i64) -> NewMessage {
    NewMessage {
        turn_id: turn_id.to_string(),
        role: MessageRole::User,
        author: None,
        source_session: None,
        timestamp: Utc::now() - Duration::days(age_days),
        text: format!("turn {turn_id}"),
        tools: Vec::new(),
        files: Vec::new(),
        parent: None,
        blocks: Vec::new(),
    }
}

fn append(session_id: Uuid, messages: Vec<NewMessage>) -> AppendMessagesRequest {
    AppendMessagesRequest {
        session_id,
        messages,
    }
}

#[tokio::test]
async fn append_needs_a_session_and_skips_known_turns() {
    let (sys, _dir) = system().await;
    let ghost = Uuid::new_v4();
    assert!(matches!(
        sys.mem.messages.append(append(ghost, vec![turn("t1", 0)])).await,
        Err(MessageError::SessionNotFound(id)) if id == ghost
    ));

    let sid = sys.mem.sessions.create(None, None).await.unwrap();
    let first = sys
        .mem
        .messages
        .append(append(sid, vec![turn("t1", 0), turn("t2", 0)]))
        .await
        .unwrap();
    assert_eq!(first.appended, [0, 1]);
    assert_eq!((first.vectorized, first.queued), (0, 0), "embeddings are off");
    let second = sys
        .mem
        .messages
        .append(append(sid, vec![turn("t2", 0), turn("t3", 0)]))
        .await
        .unwrap();
    assert_eq!(second.appended, [2]);
    assert_eq!(second.skipped, 1);
    assert!(
        sys.get_session(sid)
            .await
            .unwrap()
            .load()
            .records()
            .is_empty(),
        "messages never enter the session"
    );
}

/// The capture path stores the turns and queues their embedding: the call
/// embeds nothing itself (`vectorized` 0, `queued` the stored turns), the
/// session is checked without being loaded, and the queued work embeds
/// the turns.
#[tokio::test]
async fn append_queues_the_embedding_and_does_not_load_the_session() {
    use crate::capabilities::SearchScope;
    use crate::content_vectorizer::{ContentKind, SearchOptions};

    for backend in crate::memory_system::backends_tests::backends().await {
        let config = crate::memory_system::backends_tests::embeddings_config();
        let sid = backend
            .start_with(config.clone())
            .await
            .mem
            .sessions
            .create(None, None)
            .await
            .unwrap();
        // A restart: the session is stored, not cached.
        let sys = backend.start_with(config).await;
        let mut message = turn("t1", 0);
        message.text = "the lighthouse keeper rings the bell at dawn".into();
        let resp = sys
            .mem
            .messages
            .append(append(sid, vec![message]))
            .await
            .unwrap();
        assert_eq!((resp.vectorized, resp.queued), (0, 1), "{}", backend.name);
        assert!(
            sys.session_manager.cached(sid).is_none(),
            "{}: the append loaded the session",
            backend.name
        );
        assert!(sys.mem.records.wait_idle(std::time::Duration::from_secs(120)).await);
        let hits = sys
            .mem
            .search
            .search(
                SearchScope::Sessions(vec![sid]),
                "lighthouse keeper bell",
                SearchOptions {
                    corpus: post_cortex_ports::Corpus::Messages,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(
            hits.iter().any(|h| h.content_type == ContentKind::Message),
            "{}: the queued turn was not embedded",
            backend.name
        );
    }
}

#[tokio::test]
async fn stats_and_pages_go_through_the_actor_on_every_backend() {
    for backend in crate::memory_system::backends_tests::backends().await {
        let sys = backend.start().await;
        let sid = sys.mem.sessions.create(None, None).await.unwrap();
        assert_eq!(
            sys.mem.messages.stats(sid).await.unwrap(),
            MessageStats::default()
        );
        let turns = (0..5).map(|i| turn(&format!("t{i}"), 0)).collect();
        sys.mem.messages.store(sid, turns).await.unwrap();

        let stats = sys.mem.messages.stats(sid).await.unwrap();
        assert_eq!(
            (stats.count, stats.first_seq, stats.last_seq),
            (5, Some(0), Some(4)),
            "{}",
            backend.name
        );
        let newest = sys
            .mem.messages.page(sid, MessageCursor::Before(u64::MAX), 2, MessageOrder::Desc)
            .await
            .unwrap();
        assert_eq!(
            newest.iter().map(|m| m.seq).collect::<Vec<_>>(),
            [4, 3],
            "{}",
            backend.name
        );
        let older = sys
            .mem.messages.page(sid, MessageCursor::Before(3), 10, MessageOrder::Asc)
            .await
            .unwrap();
        assert_eq!(
            older.iter().map(|m| m.seq).collect::<Vec<_>>(),
            [0, 1, 2],
            "{}",
            backend.name
        );
        assert_eq!(stats.last_at, Some(newest[0].timestamp), "{}", backend.name);
    }
}

#[tokio::test]
async fn expiry_deletes_old_messages_and_their_vectors() {
    let (sys, _dir) = system().await;
    let sid = sys.mem.sessions.create(None, None).await.unwrap();
    sys.mem.messages.store(sid, vec![turn("old", 120), turn("new", 1)])
        .await
        .unwrap();
    for seq in [0, 1] {
        let metadata = VectorMetadata::new(
            message::content_id(seq),
            "text".to_string(),
            sid.to_string(),
            MESSAGE_CONTENT_TYPE.to_string(),
        );
        backend.vectors()
            .upsert_vectors(vec![(vec![0.5; sys.config.vector_dimension], metadata)])
            .await
            .unwrap();
    }

    let expired = sys
        .mem
        .messages
        .expire(Utc::now() - Duration::days(90))
        .await
        .unwrap();
    assert_eq!(expired, 1);
    let left = sys.stores.messages.load_messages(sid, 0..10).await.unwrap();
    assert_eq!(left.iter().map(|m| m.seq).collect::<Vec<_>>(), [1]);
    assert_eq!(
        backend.vectors()
            .count_session_embeddings(&sid.to_string())
            .await
            .unwrap(),
        1,
        "the expired message's vector is gone, the other stays"
    );
}
