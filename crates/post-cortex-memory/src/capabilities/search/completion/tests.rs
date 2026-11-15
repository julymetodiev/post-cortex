// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;
use crate::memory_system::SystemConfig;
use crate::memory_system::backends_tests::TestSystem;
use chrono::Utc;
use post_cortex_core::core::message::{MessageRole, NewMessage};

async fn system() -> (TestSystem, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let config = SystemConfig {
        enable_embeddings: false,
        ..Default::default()
    };
    (
        TestSystem::new(
            crate::memory_system::backends_tests::rocksdb_runtime(&dir, config)
                .await
                .unwrap(),
        ),
        dir,
    )
}

fn turn(turn_id: &str, _age_days: i64) -> NewMessage {
    NewMessage {
        turn_id: turn_id.to_string(),
        role: MessageRole::User,
        author: None,
        source_session: None,
        timestamp: Utc::now(),
        text: format!("turn {turn_id}"),
        tools: Vec::new(),
        files: Vec::new(),
        parent: None,
        blocks: Vec::new(),
    }
}

#[tokio::test]
async fn message_hits_come_with_whole_text_and_adjacent_turns() {
    let (sys, _dir) = system().await;
    let sid = sys.mem.sessions.create(None, None).await.unwrap();
    let turns: Vec<NewMessage> = (0..5).map(|i| turn(&format!("t{i}"), 0)).collect();
    sys.mem.messages.store(sid, turns).await.unwrap();

    let result = |content_id: &str, content_type: ContentKind, score: f32| SemanticSearchResult {
        validity: None,
        signals: None,
        document: None,
        confidence: None,
        content_id: content_id.to_string(),
        source: crate::content_vectorizer::HitSource::Session(sid),
        content_type,
        text_content: "chunk".to_string(),
        similarity_score: score,
        importance_score: 0.4,
        timestamp: Utc::now(),
        combined_score: score,
        neighbour_of: None,
        author: None,
        title: None,
        derived_from: Vec::new(),
        full_chars: None,
    };
    let hits = vec![
        result("message:2", ContentKind::Message, 0.9),
        result("some-update", ContentKind::DecisionPoint, 0.8),
        // Also a neighbour of message:2; keeps its own score.
        result("message:3", ContentKind::Message, 0.5),
    ];
    let radius = sys.rt.config.neighbour_radius;
    let out = Completion::new(Arc::clone(&sys.rt))
        .complete_message_results(hits, radius)
        .await;
    let ids: Vec<&str> = out.iter().map(|r| r.content_id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "message:1",
            "message:2",
            "message:3",
            "some-update",
            "message:4"
        ]
    );
    assert_eq!(
        out[1].text_content, "user: turn t2",
        "whole turn, not the chunk"
    );
    assert_eq!(out[0].neighbour_of.as_deref(), Some("message:2"));
    assert_eq!(out[0].similarity_score, 0.0);
    assert_eq!(out[2].similarity_score, 0.5, "a hit keeps its own score");
    assert_eq!(out[2].neighbour_of, None);
    assert_eq!(out[3].text_content, "chunk", "records pass through");
    assert_eq!(out[4].neighbour_of.as_deref(), Some("message:3"));
}
