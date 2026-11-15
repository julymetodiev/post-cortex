// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;
use chrono::{TimeZone, Utc};

fn hit(session: Uuid, content_id: &str, score: f32, at: i64) -> SemanticSearchResult {
    SemanticSearchResult {
        content_id: content_id.to_string(),
        source: crate::content_vectorizer::HitSource::Session(session),
        content_type: ContentKind::EntityDescription,
        text_content: content_id.to_string(),
        similarity_score: score,
        importance_score: 0.5,
        timestamp: Utc.timestamp_opt(at, 0).unwrap(),
        combined_score: score,
        neighbour_of: None,
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

/// Content ids repeat across sessions (entity names, message positions):
/// a global or multi-session search keeps one per (session, content id),
/// not one per content id.
#[test]
fn one_entity_name_in_two_sessions_is_two_results() {
    let (a, b) = (Uuid::from_u128(1), Uuid::from_u128(2));
    let results = dedup_and_rank(vec![
        hit(a, "entity:Rust", 0.9, 10),
        hit(b, "entity:Rust", 0.8, 10),
        // A second copy in session a (a chunk of the same content): the
        // better one stays.
        hit(a, "entity:Rust", 0.7, 10),
    ]);
    let kept: Vec<(Uuid, f32)> = results
        .iter()
        .map(|r| (r.source.id(), r.combined_score))
        .collect();
    assert_eq!(kept, [(a, 0.9), (b, 0.8)]);
}

/// The order is total: equal scores come back newest first, then by
/// session and content id, whatever order they arrived in; NaN does not
/// scramble the rest.
#[test]
fn equal_scores_have_one_order() {
    let (a, b) = (Uuid::from_u128(1), Uuid::from_u128(2));
    let input = vec![
        hit(b, "x", 0.5, 10),
        hit(a, "y", 0.5, 10),
        hit(a, "x", 0.5, 10),
        hit(a, "z", 0.5, 20),
        hit(a, "w", f32::NAN, 10),
        hit(b, "top", 0.9, 1),
    ];
    let order = |input: Vec<SemanticSearchResult>| -> Vec<(Uuid, String)> {
        dedup_and_rank(input)
            .into_iter()
            .filter(|r| !r.combined_score.is_nan())
            .map(|r| (r.source.id(), r.content_id))
            .collect()
    };
    let expected = vec![
        (b, "top".to_string()),
        (a, "z".to_string()),
        (a, "x".to_string()),
        (a, "y".to_string()),
        (b, "x".to_string()),
    ];
    assert_eq!(order(input.clone()), expected);
    let mut reversed = input;
    reversed.reverse();
    assert_eq!(order(reversed), expected);
}
