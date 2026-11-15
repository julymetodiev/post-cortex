// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Pages of a stream and search hits.

use super::*;
use crate::content_vectorizer::{ContentKind, SemanticSearchResult};
use post_cortex_core::core::message::{Message, MessageRole, MessageStats};
use std::collections::HashMap;

fn stats(first: u64, last: u64) -> MessageStats {
    MessageStats {
        count: last - first + 1,
        first_seq: Some(first),
        last_seq: Some(last),
        first_at: None,
        last_at: None,
        authors: Vec::new(),
    }
}

#[test]
fn page_neighbours_point_at_the_pages_on_either_side() {
    // A stream of 0..=9, pages of 4.
    let s = stats(0, 9);
    // From the start: a next page, no previous one.
    assert_eq!(
        neighbours(PageAt::From(0), Some(0), Some(3), true, &s),
        (Some(4), None)
    );
    // From 4: both sides.
    assert_eq!(
        neighbours(PageAt::From(4), Some(4), Some(7), true, &s),
        (Some(8), Some(4))
    );
    // The last page from 8: no next.
    assert_eq!(
        neighbours(PageAt::From(8), Some(8), Some(9), false, &s),
        (None, Some(8))
    );
    // The newest page: no next, an older one.
    assert_eq!(
        neighbours(PageAt::Before(u64::MAX), Some(6), Some(9), true, &s),
        (None, Some(6))
    );
    // An empty page past the end sits at its cursor.
    assert_eq!(
        neighbours(PageAt::From(20), None, None, false, &s),
        (None, Some(20))
    );
}

fn message(session_id: Uuid, seq: u64, text: &str) -> Message {
    Message {
        session_id,
        seq,
        turn_id: format!("t{seq}"),
        role: MessageRole::User,
        author: Some("ana".into()),
        timestamp: at(seq as i64),
        text: text.into(),
        tools: Vec::new(),
        files: Vec::new(),
        source_session: None,
        parent: None,
        blocks: Vec::new(),
    }
}

fn result(
    content_id: String,
    session_id: Uuid,
    neighbour_of: Option<&str>,
) -> SemanticSearchResult {
    SemanticSearchResult {
        content_id,
        source: crate::content_vectorizer::HitSource::Session(session_id),
        content_type: ContentKind::DecisionPoint,
        text_content: "text".into(),
        similarity_score: 0.5,
        importance_score: 0.5,
        timestamp: at(0),
        combined_score: 0.5,
        neighbour_of: neighbour_of.map(str::to_string),
        author: None,
        title: None,
        derived_from: Vec::new(),
        validity: None,
        confidence: None,
        signals: None,
        document: None,
        full_chars: None,
    }
}

#[tokio::test]
async fn search_hits_group_neighbours_and_carry_records_and_turns() {
    let old = rec(0, UpdateType::DecisionMade, "use redis").update();
    let new = rec(5, UpdateType::DecisionMade, "drop redis")
        .supersedes(&[old.id])
        .drawn_from(&["message:2"])
        .update();
    let s = scope(vec![old.clone(), new.clone()]).await;
    let session = s.sessions[0].id();
    let index = Index::of(&s);
    let turn_id = post_cortex_core::core::message::content_id(2);
    let mut hit_old = result(old.id.to_string(), session, None);
    hit_old.validity = index.validity.get(&old.id).cloned();
    let results = vec![
        // A neighbour that arrives before its hit goes before it.
        result(turn_id.clone(), session, Some(&old.id.to_string())),
        hit_old,
        result(new.id.to_string(), session, Some(&old.id.to_string())),
        result("entity:redis".into(), session, None),
    ];
    let mut turns: Turns = HashMap::new();
    turns.insert((session, 2), message(session, 2, "let us drop redis"));

    let hits = index.search_hits(results, &turns);
    assert_eq!(hits.len(), 2);
    let first = &hits[0];
    assert_eq!(first.ended_by_title.as_deref(), Some("drop redis"));
    assert!(matches!(&first.item.found, Some(Found::Record(r)) if r.id == old.id));
    assert_eq!(first.before.len(), 1);
    match &first.before[0].found {
        Some(Found::Turn(turn)) => {
            assert_eq!(turn.message.seq, 2);
            assert_eq!(turn.records, [new.id], "the record drawn from it");
        }
        other => panic!("expected a turn, got {other:?}"),
    }
    assert_eq!(first.after.len(), 1);
    // An entity description has no record to show.
    assert!(hits[1].item.found.is_none());
}
