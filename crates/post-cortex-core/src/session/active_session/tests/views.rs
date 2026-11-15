// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;
use crate::core::structured_context::MAX_FLOW_ITEMS;
use crate::summary::{SummaryGenerator, SummaryOptions};

/// A decision keeps the confidence its record states; the summary's
/// `min_confidence` filters on it and orders by it.
#[test]
fn decision_confidence_comes_from_the_record() {
    let mut s = session_with(prefs(50));
    let mut sure = update(0, UpdateType::DecisionMade, false);
    sure.meta.confidence = Some(0.9);
    let mut unsure = update(1, UpdateType::DecisionMade, false);
    unsure.meta.confidence = Some(0.3);
    let unstated = update(2, UpdateType::DecisionMade, false);
    for u in [sure, unsure, unstated] {
        s.apply_record(u);
    }
    let options = SummaryOptions {
        min_confidence: Some(0.5),
        ..Default::default()
    };
    let view =
        SummaryGenerator::generate_structured_summary_filtered(&s, &options, chrono::Utc::now());
    let kept: Vec<(&str, f32)> = view
        .key_decisions
        .iter()
        .map(|d| (d.description.as_str(), d.confidence))
        .collect();
    assert_eq!(kept, [("title 2", 1.0), ("title 0", 0.9)]);
    let insights = SummaryGenerator::extract_key_insights(&s, 10);
    assert!(
        !insights.iter().any(|i| i.contains("title 1")),
        "{insights:?}"
    );
}

/// The conversation flow stays within `MAX_FLOW_ITEMS` however many
/// records the session has.
#[test]
fn conversation_flow_is_bounded() {
    let mut s = session_with(prefs(5));
    for i in 0..(MAX_FLOW_ITEMS as u32 + 20) {
        s.apply_record(update(i, UpdateType::ProblemSolved, false));
    }
    assert!(s.current_state().conversation_flow.len() <= MAX_FLOW_ITEMS);
}

/// The views are the records': applying records one by one and loading
/// them at once give the same state, flow, code references and graph.
#[test]
fn views_derive_from_the_records_alone() {
    let s = populated_session();
    let loaded = ActiveSession::from_records(
        s.metadata().clone(),
        s.last_updated(),
        s.records_owned(),
        SimpleEntityGraph::new(),
    );
    assert_eq!(loaded.current_state(), s.current_state());
    assert_eq!(
        serde_json::to_value(loaded.code_references()).unwrap(),
        serde_json::to_value(s.code_references()).unwrap()
    );
    let state = s.current_state();
    assert_eq!(state.answered_questions.len(), 1);
    assert_eq!(state.key_decisions.len(), 1);
    assert_eq!(state.key_concepts.len(), 1);
    assert_eq!(state.technical_specifications.len(), 1);
    assert_eq!(state.conversation_flow.len(), 5);
    assert_eq!(s.code_references()["src/lib.rs"].len(), 3);
    assert_eq!(s.change_history().len(), 5);
    let hot: Vec<&str> = s.hot_records().map(|u| u.content.title.as_str()).collect();
    assert_eq!(hot, ["title 2", "title 3", "title 4"]);
    assert_eq!(s.last_updated(), s.records()[4].timestamp);
}
