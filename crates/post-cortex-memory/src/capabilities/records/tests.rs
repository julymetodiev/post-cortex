// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Records over a running memory instance: writes, bulk writes, the
//! background pipeline, entity embeddings and message provenance.

use super::*;
use crate::capabilities::test_support::rocksdb_memory;
use crate::{capabilities::Memory, memory_system::SystemConfig};
use post_cortex_core::core::message::{self, MessageRole, NewMessage};
use post_cortex_core::services::AppendMessagesRequest;

/// An instance with embeddings on (the default configuration).
async fn embedding_memory() -> (Memory, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let memory = crate::memory_system::backends_tests::rocksdb_memory_in(
        &dir,
        SystemConfig {
            ..Default::default()
        },
    )
    .await
    .unwrap();
    (memory, dir)
}

fn write_to(session_id: Uuid) -> UpdateContextRequest {
    let mut req = good_request();
    req.session_id = session_id;
    req
}

#[tokio::test]
async fn a_write_is_stored_as_the_record_it_describes() {
    let (memory, _dir) = rocksdb_memory().await;
    let sid = memory.sessions.create(None, None).await.unwrap();
    let mut req = write_to(sid);
    req.author = Some("claude".into());
    let resp = memory.records.write(req).await.unwrap();
    assert_eq!(resp.session_id, sid);
    assert!(resp.durable);

    let session = memory.sessions.get(sid).await.unwrap();
    let records = session.records();
    assert_eq!(records.len(), 1, "one record, nothing fabricated");
    let record = &records[0];
    assert_eq!(record.id, resp.entry_id);
    assert_eq!(record.content.title, "Some concept");
    assert_eq!(record.content.description, "A short definition");
    assert_eq!(record.author.as_deref(), Some("claude"));
}

#[tokio::test]
async fn invalid_input_and_unknown_sessions_are_typed() {
    let (memory, _dir) = rocksdb_memory().await;
    let sid = memory.sessions.create(None, None).await.unwrap();
    let mut req = write_to(sid);
    req.entities = vec![];
    assert!(matches!(
        memory.records.write(req).await,
        Err(RecordError::InvalidArgument(_))
    ));
    let ghost = Uuid::new_v4();
    assert!(matches!(
        memory.records.write(write_to(ghost)).await,
        Err(RecordError::SessionNotFound(id)) if id == ghost
    ));
}

#[tokio::test]
async fn bulk_writes_store_every_item_and_refuse_mismatched_sessions() {
    let (memory, _dir) = rocksdb_memory().await;
    let sid = memory.sessions.create(None, None).await.unwrap();
    let mut a = write_to(sid);
    a.content.title = "First".into();
    let mut b = write_to(sid);
    b.content.title = "Second".into();
    let resp = memory
        .records
        .bulk_write(BulkUpdateContextRequest {
            session_id: sid,
            updates: vec![a, b],
        })
        .await
        .unwrap();
    assert_eq!(resp.entry_ids.len(), 2);
    assert!(resp.durable);

    let err = memory
        .records
        .bulk_write(BulkUpdateContextRequest {
            session_id: sid,
            updates: vec![write_to(Uuid::new_v4())],
        })
        .await
        .unwrap_err();
    assert!(matches!(err, RecordError::InvalidArgument(ref m) if m.contains("does not match")));
}

/// The write returns once stored; embedding runs on the pipeline and its
/// backlog drains.
#[tokio::test]
async fn writes_return_fast_then_the_pipeline_drains() {
    let (memory, _dir) = embedding_memory().await;
    let sid = memory.sessions.create(None, None).await.unwrap();
    memory
        .admin
        .warm_up(&tokio_util::sync::CancellationToken::new())
        .await
        .unwrap();
    memory.records.write(write_to(sid)).await.unwrap();
    assert!(memory.records.wait_idle(Duration::from_secs(30)).await);

    let start = std::time::Instant::now();
    let resp = memory.records.write(write_to(sid)).await.unwrap();
    let latency = start.elapsed();
    assert!(resp.durable);
    assert!(
        latency.as_millis() < 250,
        "write took {latency:?} on the warm path"
    );
    assert!(memory.records.wait_idle(Duration::from_secs(30)).await);
    assert_eq!(memory.admin.health().pipeline_backlog, 0);
}

/// Each write merges its entities into the graph once.
#[tokio::test]
async fn writes_merge_entities_exactly_once() {
    let (memory, _dir) = rocksdb_memory().await;
    let sid = memory.sessions.create(None, None).await.unwrap();
    for round in 1..=3u32 {
        assert!(memory.records.write(write_to(sid)).await.unwrap().durable);
        assert!(memory.records.wait_idle(Duration::from_secs(10)).await);
        let session = memory.sessions.get(sid).await.unwrap();
        assert_eq!(
            session.entity_graph().entities["Foo"].mention_count,
            round,
            "one merge per write"
        );
    }
}

/// The entities a write names get vectors from the write itself: search
/// finds them without a vectorize pass.
#[tokio::test]
async fn entities_of_a_write_are_embedded() {
    let (memory, _dir) = embedding_memory().await;
    let sid = memory.sessions.create(None, None).await.unwrap();
    memory.records.write(write_to(sid)).await.unwrap();
    assert!(memory.records.wait_idle(Duration::from_secs(120)).await);
    let hits = memory
        .search
        .search(
            crate::capabilities::SearchScope::Session(sid),
            "Foo",
            Default::default(),
        )
        .await
        .unwrap();
    assert!(
        hits.iter().any(|h| h.content_id == "entity:Foo"),
        "entity:Foo has no vector"
    );
}

/// `derived_from` must name messages stored in the session; the links come
/// back on the record's search hits.
#[tokio::test]
async fn derived_from_names_stored_messages() {
    let (memory, _dir) = embedding_memory().await;
    let sid = memory.sessions.create(None, None).await.unwrap();
    let appended = memory
        .messages
        .append(AppendMessagesRequest {
            session_id: sid,
            messages: vec![NewMessage {
                turn_id: "t1".to_string(),
                role: MessageRole::User,
                author: None,
                source_session: None,
                timestamp: Utc::now(),
                text: "Let us keep the cards in front of every session".to_string(),
                tools: Vec::new(),
                files: Vec::new(),
                parent: None,
                blocks: Vec::new(),
            }],
        })
        .await
        .unwrap();
    let seq = appended.appended[0];
    let id = message::content_id(seq);

    let with = |derived_from: Vec<String>| {
        let mut req = write_to(sid);
        req.derived_from = derived_from;
        req
    };
    for bad in [
        vec!["entity:Foo".to_string()],
        vec![format!("{id}#1")],
        vec![message::content_id(seq + 7)],
    ] {
        assert!(matches!(
            memory.records.write(with(bad)).await,
            Err(RecordError::InvalidArgument(_))
        ));
    }
    memory.records.write(with(vec![id.clone()])).await.unwrap();
    assert!(memory.records.wait_idle(Duration::from_secs(120)).await);

    let hits = memory
        .search
        .search(
            crate::capabilities::SearchScope::Session(sid),
            "Some concept short definition",
            Default::default(),
        )
        .await
        .unwrap();
    let record = hits
        .iter()
        .find(|h| h.content_type == crate::content_vectorizer::ContentKind::UpdateContent)
        .expect("the record is found");
    assert_eq!(record.derived_from, [id]);
}
