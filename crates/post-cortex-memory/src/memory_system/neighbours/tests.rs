// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;
use crate::content_vectorizer::ContentKind;
use post_cortex_core::core::context_update::{UpdateContent, UpdateType};

fn update(title: &str) -> ContextUpdate {
    ContextUpdate {
        meta: Default::default(),
        id: Uuid::new_v4(),
        update_type: UpdateType::QuestionAnswered,
        content: UpdateContent {
            title: title.to_string(),
            description: format!("{title} said something"),
            details: Vec::new(),
            examples: Vec::new(),
            implications: Vec::new(),
        },
        timestamp: chrono::Utc::now(),
        related_code: None,
        parent_update: None,
        user_marked_important: false,
        creates_entities: Vec::new(),
        creates_relationships: Vec::new(),
        references_entities: Vec::new(),
        typed_entities: Vec::new(),
        author: None,
        derived_from: Vec::new(),
    }
}

async fn session(n: usize) -> Arc<ActiveSession> {
    let mut s = ActiveSession::new(Uuid::new_v4(), None, None, chrono::Utc::now());
    for i in 0..n {
        s.apply_record(update(&format!("turn {i}")));
    }
    Arc::new(s)
}

fn hit(s: &ActiveSession, i: usize, score: f32) -> SemanticSearchResult {
    let u = &s.records()[i];
    SemanticSearchResult {
        validity: None,
        signals: None,
        document: None,
        confidence: None,
        content_id: u.id.to_string(),
        source: crate::content_vectorizer::HitSource::Session(s.id()),
        content_type: ContentKind::UserMessage,
        text_content: u.content.title.clone(),
        similarity_score: score,
        importance_score: 0.8,
        timestamp: u.timestamp,
        combined_score: score,
        neighbour_of: None,
        author: None,
        title: None,
        derived_from: Vec::new(),
        full_chars: None,
    }
}

fn titles(rs: &[SemanticSearchResult]) -> Vec<String> {
    rs.iter()
        .map(|r| {
            let t = r
                .text_content
                .split_whitespace()
                .take(2)
                .collect::<Vec<_>>()
                .join(" ");
            match &r.neighbour_of {
                Some(_) => format!("({t})"),
                None => t,
            }
        })
        .collect()
}

#[tokio::test]
async fn hits_come_with_adjacent_updates_once() {
    let s = session(10).await;
    let sessions = HashMap::from([(s.id(), Arc::clone(&s))]);
    // Hits at positions 5, 0 and 6: 6 is also 5's neighbour, so it keeps
    // its hit scores and is not repeated; 0 has no left neighbour.
    let hits = vec![hit(&s, 5, 0.9), hit(&s, 0, 0.8), hit(&s, 6, 0.7)];
    let out = complete_results(hits, &sessions, 1, &ValidityCache::default());
    assert_eq!(
        titles(&out),
        [
            "(turn 4)", "turn 5", "turn 6", "turn 0", "(turn 1)", "(turn 7)"
        ]
    );
    let n4 = &out[0];
    assert_eq!(n4.neighbour_of.as_deref(), Some(out[1].content_id.as_str()));
    assert_eq!(n4.combined_score, 0.9);
    assert_eq!(n4.similarity_score, 0.0);
    assert_eq!(out[2].similarity_score, 0.7, "hit 6 keeps its own scores");
    assert!(out[0].text_content.contains("turn 4 said something"));
}

#[tokio::test]
async fn long_hits_get_their_full_text_back() {
    let mut s = ActiveSession::new(Uuid::new_v4(), None, None, chrono::Utc::now());
    let mut long = update("long");
    long.content.description = "word ".repeat(1000);
    s.apply_record(long);
    let s = Arc::new(s);
    let sessions = HashMap::from([(s.id(), Arc::clone(&s))]);

    let mut hits = vec![hit(&s, 0, 0.9)];
    hits[0].text_content = "long | word word ...".into(); // the embedded summary
    let hits = complete_results(hits, &sessions, 0, &ValidityCache::default());
    assert_eq!(hits[0].text_content.len(), "long ".len() + 5000);

    // Text already longer than the update's (e.g. graph enrichment) stays.
    let mut enriched = vec![hit(&s, 0, 0.9)];
    enriched[0].text_content = "x".repeat(6000);
    let enriched = complete_results(enriched, &sessions, 0, &ValidityCache::default());
    assert_eq!(enriched[0].text_content.len(), 6000);
}

#[tokio::test]
async fn radius_zero_unknown_sessions_and_entities_pass_through() {
    let s = session(3).await;
    let sessions = HashMap::from([(s.id(), Arc::clone(&s))]);
    let hits = vec![hit(&s, 1, 0.9)];
    assert_eq!(
        complete_results(hits.clone(), &sessions, 0, &ValidityCache::default()).len(),
        1
    );
    assert_eq!(
        complete_results(hits.clone(), &HashMap::new(), 1, &ValidityCache::default()).len(),
        1
    );

    let mut entity = hit(&s, 1, 0.5);
    entity.content_id = "entity:rust".into();
    entity.content_type = ContentKind::EntityDescription;
    let out = complete_results(vec![entity], &sessions, 1, &ValidityCache::default());
    assert_eq!(out.len(), 1);
    assert!(out[0].neighbour_of.is_none());
}

/// Records a later record superseded, or written with low confidence,
/// rank lower and say so; the heuristically restated one is only marked.
#[tokio::test]
async fn ended_and_uncertain_records_rank_lower() {
    let mut s = ActiveSession::new(Uuid::new_v4(), None, None, chrono::Utc::now());
    let old = update("old plan");
    let mut new = update("new plan");
    new.meta.supersedes = vec![old.id];
    let mut unsure = update("maybe later");
    unsure.meta.confidence = Some(0.5);
    for u in [old.clone(), new.clone(), unsure.clone()] {
        s.apply_record(u);
    }
    let s = Arc::new(s);
    let sessions = HashMap::from([(s.id(), Arc::clone(&s))]);
    let hits = vec![hit(&s, 0, 0.9), hit(&s, 2, 0.85), hit(&s, 1, 0.6)];
    let out = complete_results(hits, &sessions, 0, &ValidityCache::default());
    assert_eq!(titles(&out), ["maybe later", "new plan", "old plan"]);
    let old_hit = &out[2];
    let v = old_hit.validity.as_ref().expect("ended");
    assert_eq!(v.ended_by, Some(new.id));
    assert_eq!(v.reason, Some(EndReason::Superseded));
    assert!((old_hit.combined_score - 0.45).abs() < 1e-6);
    assert_eq!(out[0].confidence, Some(0.5));
    assert!((out[0].combined_score - 0.6375).abs() < 1e-6);
    assert!(out[1].validity.is_none());
}
