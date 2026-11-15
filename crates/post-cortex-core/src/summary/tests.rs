// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;
use crate::session::active_session::ActiveSession;
use uuid::Uuid;

#[test]
fn test_summary_generation() {
    let session = ActiveSession::new(
        Uuid::new_v4(),
        Some("Test Session".to_string()),
        Some("Test session for summary generation".to_string()),
        Utc::now(),
    );

    let summary = SummaryGenerator::generate_structured_summary(&session, Utc::now());

    assert_eq!(summary.session_id, session.id());
    assert!(summary.generated_at <= Utc::now());
    assert_eq!(summary.session_stats.hot_context_size, 0); // Empty session
}

#[test]
fn test_key_insights_extraction() {
    let session = ActiveSession::new(
        Uuid::new_v4(),
        Some("Test Session".to_string()),
        Some("Test session for insights".to_string()),
        Utc::now(),
    );

    let insights = SummaryGenerator::extract_key_insights(&session, 5);

    // Empty session should have minimal insights
    assert!(insights.len() <= 5);
}

#[test]
fn test_limited_summary_matches_full_sort_then_truncate() {
    use crate::core::context_update::EntityType;
    use crate::core::context_update::{ContextUpdate, UpdateContent, UpdateType};
    use crate::core::structured_context::{ConceptItem, DecisionItem, QuestionItem};
    use chrono::TimeZone;

    let ts = |s: u32| Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, s).unwrap();
    let mut session = ActiveSession::new(Uuid::new_v4(), None, None, ts(0));
    let record = |kind: UpdateType, title: String, at| ContextUpdate {
        meta: Default::default(),
        id: Uuid::new_v4(),
        timestamp: at,
        update_type: kind,
        content: UpdateContent {
            title,
            description: String::new(),
            details: vec![],
            examples: vec![],
            implications: vec![],
        },
        related_code: None,
        parent_update: None,
        user_marked_important: false,
        creates_entities: vec![],
        creates_relationships: vec![],
        references_entities: vec![],
        typed_entities: vec![],
        author: None,
        derived_from: vec![],
    };
    for i in 0..30u32 {
        let mut decision = record(UpdateType::DecisionMade, format!("d{i}"), ts(i % 7));
        // Record 3's confidence is NaN: must not panic.
        decision.meta.confidence = Some(if i == 3 {
            f32::NAN
        } else {
            (i % 4) as f32 / 4.0
        });
        session.apply_record(decision);
    }
    for i in 0..30u32 {
        session.apply_record(record(
            UpdateType::QuestionAnswered,
            format!("q{i}"),
            ts(i % 5),
        ));
    }
    for i in 0..30u32 {
        session.apply_record(record(
            UpdateType::ConceptDefined,
            format!("c{i}"),
            ts(i % 3),
        ));
    }
    let graph = session.entity_graph_mut();
    for i in 0..25u32 {
        graph.add_or_update_entity(format!("e{i:02}"), EntityType::Concept, ts(0), "");
        // No relations: importance follows the mentions.
        graph
            .entities
            .get_mut(&format!("e{i:02}"))
            .unwrap()
            .mention_count = i % 3 + 1;
    }

    let options = SummaryOptions {
        decisions_limit: Some(5),
        entities_limit: Some(4),
        questions_limit: Some(6),
        concepts_limit: Some(7),
        min_confidence: Some(0.25),
        compact: false,
    };
    let view =
        SummaryGenerator::generate_structured_summary_filtered(&session, &options, Utc::now());

    // Reference: the previous full stable sort + truncate.
    let state = session.current_state();
    let mut decisions: Vec<&DecisionItem> = state
        .key_decisions
        .iter()
        .filter(|d| d.confidence >= 0.25)
        .collect();
    decisions.sort_by(|a, b| {
        b.confidence
            .total_cmp(&a.confidence)
            .then_with(|| b.timestamp.cmp(&a.timestamp))
    });
    let expected: Vec<&str> = decisions
        .iter()
        .take(5)
        .map(|d| d.description.as_str())
        .collect();
    let got: Vec<&str> = view
        .key_decisions
        .iter()
        .map(|d| d.description.as_str())
        .collect();
    assert_eq!(got, expected);

    let mut questions: Vec<&QuestionItem> = state.answered_questions.iter().collect();
    questions.sort_by_key(|r| std::cmp::Reverse(r.timestamp));
    let expected: Vec<&str> = questions
        .iter()
        .take(6)
        .map(|q| q.question.as_str())
        .collect();
    let got: Vec<&str> = view
        .answered_questions
        .iter()
        .map(|q| q.question.as_str())
        .collect();
    assert_eq!(got, expected);

    let mut concepts: Vec<&ConceptItem> = state.key_concepts.iter().collect();
    concepts.sort_by_key(|r| std::cmp::Reverse(r.timestamp));
    let expected: Vec<&str> = concepts.iter().take(7).map(|c| c.name.as_str()).collect();
    let got: Vec<&str> = view.key_concepts.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(got, expected);

    let mut all = session.entity_graph().analyze_entity_importance();
    all.truncate(4);
    let expected: Vec<String> = all.into_iter().map(|a| a.entity_name).collect();
    assert_eq!(view.important_entities, expected);
    assert_eq!(view.important_entities, vec!["e02", "e05", "e08", "e11"]);
}

/// Ages and recency are measured from the `now` the caller passes, not the
/// clock (review core-27): the same input gives the same summary.
#[test]
fn summaries_are_measured_from_the_given_now() {
    use crate::graph::entity_graph::EntityAnalysis;
    use chrono::TimeZone;

    let seen = Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap();
    let analysis = EntityAnalysis {
        entity_name: "cards".into(),
        importance_score: 0.5,
        mention_count: 1,
        relationship_count: 0,
        update_references: 1,
        first_seen: seen,
        last_seen: seen,
    };
    let at = |days| seen + chrono::Duration::days(days);
    let level = |days| {
        format!(
            "{:?}",
            EntitySummary::from_entity_analysis(&analysis, at(days)).recency_level
        )
    };
    assert_eq!(level(0), "Today");
    assert_eq!(level(2), "Recent");
    assert_eq!(level(40), "Old");

    let session = ActiveSession::new(Uuid::new_v4(), None, None, seen);
    let summary = SummaryGenerator::generate_structured_summary(&session, at(3));
    assert_eq!(summary.generated_at, at(3));
}
