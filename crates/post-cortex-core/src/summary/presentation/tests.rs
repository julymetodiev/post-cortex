// Copyright (c) 2025 Julius ML
// Permission is hereby granted, free of charge, to any person obtaining a copy

use super::*;

#[test]
fn test_confidence_level_mapping() {
    let decision = DecisionItem {
        description: "Test decision".to_string(),
        context: "Test context".to_string(),
        alternatives: vec![],
        confidence: 0.9,
        timestamp: Utc::now(),
    };

    let summary = DecisionSummary::from_decision_item(&decision);
    assert!(matches!(summary.confidence_level, ConfidenceLevel::High));
}

#[test]
fn test_urgency_level_calculation() {
    let old_timestamp = Utc::now() - chrono::Duration::days(10);
    let question = QuestionItem {
        question: "Test question".to_string(),
        context: "Test context".to_string(),
        status: QuestionStatus::Open,
        timestamp: old_timestamp,
        last_updated: old_timestamp,
    };

    let summary = QuestionSummary::from_question_item(&question, Utc::now());
    assert!(summary.days_open >= 10);
    assert!(matches!(summary.urgency_level, UrgencyLevel::High));
}

#[test]
fn test_session_duration_calculation() {
    let created = Utc::now() - chrono::Duration::hours(3);
    let updated = Utc::now();

    let stats = SessionStatsBuilder::new(Uuid::new_v4(), created, updated)
        .with_context_sizes(10, 5, 2)
        .with_counts(15, 20, 3)
        .with_references(2, 5, 1)
        .build();

    assert!(matches!(stats.session_duration, SessionDuration::Medium));
}
