// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Chunked message vectors survive an export and an import, and damaged
//! ones are repaired by `vectorize_session`.

use super::MemoryRuntime;
use super::backends_tests::{backends, embeddings_config};
use crate::memory_system::backends_tests::TestSystem;
use chrono::Utc;
use uuid::Uuid;

/// A captured turn with `text`.
fn chat(turn_id: &str, text: String) -> post_cortex_core::core::message::NewMessage {
    post_cortex_core::core::message::NewMessage {
        turn_id: turn_id.to_string(),
        role: post_cortex_core::core::message::MessageRole::Assistant,
        author: Some("claude-code".to_string()),
        source_session: None,
        timestamp: Utc::now(),
        text,
        tools: Vec::new(),
        files: Vec::new(),
        parent: None,
        blocks: Vec::new(),
    }
}

/// A text the vectorizer cuts into several chunks.
fn long_text(topic: &str) -> String {
    (0..400)
        .map(|i| format!("{topic} sentence {i}. "))
        .collect()
}

/// The persisted message vectors of `session`: `(content id, text, seq,
/// parent)`, by content id.
async fn persisted_message_vectors(
    sys: &TestSystem,
    session: Uuid,
) -> Vec<(String, String, String, Option<String>)> {
    let mut found: Vec<_> = sys
        .stored()
        .get_session_vectors(&session.to_string())
        .await
        .unwrap()
        .into_iter()
        .filter(|(_, m)| m.content_type == post_cortex_ports::MESSAGE_CONTENT_TYPE)
        .map(|(_, m)| {
            (
                m.id.clone(),
                m.text.to_string(),
                m.metadata["seq"].clone(),
                m.metadata.get("parent").cloned(),
            )
        })
        .collect();
    found.sort();
    found
}

/// The message vectors of `session` in the index, as [`persisted_message_vectors`].
async fn memory_message_vectors(
    sys: &MemoryRuntime,
    session: Uuid,
) -> Vec<(String, String, String, Option<String>)> {
    let vectorizer = sys.content_vectorizer.get().unwrap();
    let mut found: Vec<_> = vectorizer
        .index()
        .entries(&session.to_string(), &post_cortex_ports::EntrySelect::All)
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.meta)
        .filter(|m| m.content_type == post_cortex_ports::MESSAGE_CONTENT_TYPE)
        .map(|m| {
            (
                m.id.clone(),
                m.text.to_string(),
                m.metadata["seq"].clone(),
                m.metadata.get("parent").cloned(),
            )
        })
        .collect();
    found.sort();
    found
}

/// Long messages embedded as several chunks (the real `vectorize_messages`
/// path) keep every chunk through an export and an import into a fresh
/// system, and through a merge that moves them to later positions. A
/// target whose message vectors were damaged (a chunk missing, another
/// text's vectors at a position, vectors of a position no message has) is
/// repaired by `vectorize_session`, and only what was wrong is embedded.
#[tokio::test]
async fn chunked_message_vectors_survive_import_and_are_repaired() {
    use post_cortex_core::archive::{ExportOptions, ExportScope, ImportOptions};

    let sources = backends().await;
    let targets = backends().await;
    let others = backends().await;
    for ((source_backend, target_backend), other_backend) in
        sources.iter().zip(&targets).zip(&others)
    {
        let name = target_backend.name;
        let source = source_backend.start_with(embeddings_config()).await;
        let sid = source
            .mem
            .sessions
            .create(Some("s".into()), None)
            .await
            .unwrap();
        let queued = capture(
            &source,
            sid,
            vec![
                chat("a", "a short first turn".to_string()),
                chat("b", long_text("lighthouse")),
                chat("c", long_text("keeper")),
            ],
        )
        .await;
        assert_eq!(queued, 3, "{name}");
        let expected = persisted_message_vectors(&source, sid).await;
        let chunks_of = |seq: &str| expected.iter().filter(|v| v.2 == seq).count();
        assert!(
            chunks_of("1") > 2 && chunks_of("2") > 2,
            "{name}: {expected:?}"
        );
        assert_eq!(
            memory_message_vectors(&source, sid).await,
            expected,
            "{name}"
        );

        let all = source
            .mem
            .transfer
            .export_data(ExportScope::All, ExportOptions::default())
            .await
            .unwrap();
        let exported = all
            .embeddings
            .iter()
            .filter(|e| e.content_type == post_cortex_ports::MESSAGE_CONTENT_TYPE)
            .count();
        assert_eq!(exported, expected.len(), "{name}");

        // Into a fresh system: every chunk, as it was, nothing embedded.
        let target = target_backend.start_with(embeddings_config()).await;
        let result = target
            .mem
            .transfer
            .import_data(all.clone(), ImportOptions::default())
            .await
            .unwrap();
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        assert_eq!(
            (
                result.messages_imported,
                result.embeddings_imported >= expected.len()
            ),
            (3, true),
            "{name}"
        );
        assert_eq!(
            persisted_message_vectors(&target, sid).await,
            expected,
            "{name}"
        );
        assert_eq!(
            memory_message_vectors(&target, sid).await,
            expected,
            "{name}"
        );
        assert_eq!(target.vectorize_session_messages(sid).await.unwrap(), 0);

        // Into a system whose stream has another turn first: one later.
        let other = other_backend.start_with(embeddings_config()).await;
        other
            .mem
            .transfer
            .import_data(
                source
                    .mem
                    .transfer
                    .export_data(
                        ExportScope::Sessions(vec![sid]),
                        ExportOptions {
                            include_messages: false,
                            ..Default::default()
                        },
                    )
                    .await
                    .unwrap(),
                ImportOptions::default(),
            )
            .await
            .unwrap();
        capture(
            &other,
            sid,
            vec![chat("x", "the other system's own turn".into())],
        )
        .await;
        let merged = other
            .mem
            .transfer
            .import_data(
                all.clone(),
                ImportOptions {
                    skip_existing: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(merged.errors.is_empty(), "{:?}", merged.errors);
        let shifted: Vec<_> = expected
            .iter()
            .map(|(id, text, seq, parent)| {
                let to = (seq.parse::<u64>().unwrap() + 1).to_string();
                let at = id.find('#').map_or("", |i| &id[i..]);
                (
                    format!("message:{to}{at}"),
                    text.clone(),
                    to.clone(),
                    parent.as_ref().map(|_| format!("message:{to}")),
                )
            })
            .chain(std::iter::once((
                "message:0".to_string(),
                "the other system's own turn".to_string(),
                "0".to_string(),
                None,
            )))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        assert_eq!(
            persisted_message_vectors(&other, sid).await,
            shifted,
            "{name}"
        );
        assert_eq!(memory_message_vectors(&other, sid).await, shifted, "{name}");

        // Damage the target: drop a chunk of b, put another text's vectors
        // at c's position, and leave vectors where no message is.
        let vectorizer = target.content_vectorizer.get().unwrap().clone();
        // `message:1#2` names no chunks of its own: only that one goes.
        vectorizer
            .remove_contents(sid, &["message:1#2".to_string()])
            .await
            .unwrap();
        let dim = 256;
        let stale = |id: &str, seq: &str| {
            post_cortex_ports::VectorMetadata::new(
                id.to_string(),
                format!("text of another stream at {id}"),
                sid.to_string(),
                post_cortex_ports::MESSAGE_CONTENT_TYPE.to_string(),
            )
            .with_metadata("seq".into(), seq.to_string())
        };
        vectorizer
            .remove_contents(sid, &["message:2".to_string()])
            .await
            .unwrap();
        for meta in [
            stale("message:2", "2"),
            stale("message:40", "40"),
            stale("message:40#2", "40").with_metadata("parent".into(), "message:40".into()),
        ] {
            vectorizer
                .index()
                .upsert(vec![post_cortex_ports::IndexEntry {
                    vector: vec![0.5; dim],
                    meta,
                }])
                .await
                .unwrap();
        }
        assert_ne!(
            persisted_message_vectors(&target, sid).await,
            expected,
            "{name}"
        );

        let added = target.mem.admin.vectorize_session(sid).await.unwrap();
        assert_eq!(added, chunks_of("1") + chunks_of("2"), "{name}");
        assert_eq!(
            persisted_message_vectors(&target, sid).await,
            expected,
            "{name}"
        );
        assert_eq!(
            memory_message_vectors(&target, sid).await,
            expected,
            "{name}"
        );
        assert_eq!(target.vectorize_session_messages(sid).await.unwrap(), 0);

        // And a restart loads the repaired set.
        drop(target);
        let restarted = target_backend.start_with(embeddings_config()).await;
        restarted.vectorize_session_messages(sid).await.unwrap();
        assert_eq!(
            memory_message_vectors(&restarted, sid).await,
            expected,
            "{name}"
        );
        assert_eq!(
            restarted
                .mem
                .admin
                .vectorize_all_messages()
                .await
                .unwrap()
                .0,
            0,
            "{name}"
        );
    }
}

/// Append `messages` through the capture path and wait for their queued
/// embedding; returns how many turns were queued.
async fn capture(
    sys: &super::backends_tests::TestSystem,
    session_id: uuid::Uuid,
    messages: Vec<post_cortex_core::core::message::NewMessage>,
) -> usize {
    let resp = sys
        .mem
        .messages
        .append(post_cortex_core::services::AppendMessagesRequest {
            session_id,
            messages,
        })
        .await
        .unwrap();
    assert!(
        sys.mem
            .records
            .wait_idle(std::time::Duration::from_secs(120))
            .await
    );
    resp.queued
}
