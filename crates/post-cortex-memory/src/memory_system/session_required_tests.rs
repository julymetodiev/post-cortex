// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Writes need an existing session: a write, a bulk write, a message append
//! or a file invalidation for an unknown or deleted session fails with
//! not-found and never creates the session, on every backend.

use chrono::Utc;
use uuid::Uuid;

use post_cortex_core::core::message::{MessageRole, NewMessage};
use post_cortex_core::services::{AppendMessagesRequest, BulkUpdateContextRequest};

use super::backends_tests::{backends, request};
use super::write_path_tests::text_record;
use crate::capabilities::{Memory, MessageError, RecordError};

fn turn() -> NewMessage {
    NewMessage {
        turn_id: "t1".into(),
        role: MessageRole::User,
        author: None,
        source_session: None,
        timestamp: Utc::now(),
        text: "hello".into(),
        tools: Vec::new(),
        files: Vec::new(),
        parent: None,
        blocks: Vec::new(),
    }
}

/// Every write path refuses `sid` with not-found and leaves storage without it.
async fn assert_writes_refused(
    memory: &super::backends_tests::TestSystem,
    service: &Memory,
    sid: Uuid,
    backend: &str,
) {
    let not_found = |e: &RecordError| matches!(e, RecordError::SessionNotFound(id) if *id == sid);

    let err = service
        .records
        .write(request(sid, "Late write", "lands nowhere"))
        .await
        .unwrap_err();
    assert!(not_found(&err), "{backend}: write: {err:?}");

    let err = service
        .records
        .bulk_write(BulkUpdateContextRequest {
            session_id: sid,
            updates: vec![request(sid, "Late bulk", "lands nowhere")],
        })
        .await
        .unwrap_err();
    assert!(not_found(&err), "{backend}: bulk: {err:?}");

    let err = service
        .messages
        .append(AppendMessagesRequest {
            session_id: sid,
            messages: vec![turn()],
        })
        .await
        .unwrap_err();
    assert!(
        matches!(err, MessageError::SessionNotFound(id) if id == sid),
        "{backend}: messages: {err:?}"
    );

    assert!(
        memory
            .mem
            .records
            .commit(sid, text_record("direct"), true)
            .await
            .is_err(),
        "{backend}: direct write"
    );
    assert!(
        memory
            .mem
            .freshness
            .invalidate(sid, "src/lib.rs")
            .await
            .is_err(),
        "{backend}: invalidation"
    );

    assert!(
        memory
            .session_manager
            .load_stored(sid)
            .await
            .unwrap()
            .is_none(),
        "{backend}: the session was created"
    );
    assert!(memory.get_session(sid).await.is_err(), "{backend}");
}

#[tokio::test]
async fn writes_to_an_unknown_session_fail_and_create_nothing() {
    for backend in backends().await {
        let memory = backend.start().await;
        let service = crate::capabilities::test_support::memory(&memory.clone());
        assert_writes_refused(&memory, &service, Uuid::new_v4(), backend.name).await;
    }
}

#[tokio::test]
async fn writes_to_a_deleted_session_fail_and_do_not_bring_it_back() {
    for backend in backends().await {
        let memory = backend.start().await;
        let service = crate::capabilities::test_support::memory(&memory.clone());
        let sid = memory.mem.sessions.create(None, None).await.unwrap();
        service
            .records
            .write(request(sid, "First", "before the delete"))
            .await
            .unwrap();
        assert!(memory.mem.sessions.delete(sid).await.unwrap());

        assert_writes_refused(&memory, &service, sid, backend.name).await;
    }
}
