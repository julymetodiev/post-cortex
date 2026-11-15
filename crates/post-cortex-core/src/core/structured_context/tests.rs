// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

#[test]
fn test_decision_item_confidence_clamping() {
    let item = DecisionItem::new(
        "Test".to_string(),
        "Context".to_string(),
        vec![],
        1.5, // Should be clamped to 1.0
        Utc::now(),
    );
    assert_eq!(item.confidence, 1.0);

    let item2 = DecisionItem::new(
        "Test".to_string(),
        "Context".to_string(),
        vec![],
        -0.5, // Should be clamped to 0.0
        Utc::now(),
    );
    assert_eq!(item2.confidence, 0.0);
}

#[test]
fn test_flow_item_limit() {
    let mut ctx = StructuredContext::new();

    // Add MAX_FLOW_ITEMS + 100 items
    for i in 0..(MAX_FLOW_ITEMS + 100) {
        ctx.add_flow_item(FlowItem {
            step_description: format!("Step {}", i),
            timestamp: Utc::now(),
            related_updates: Vec::new(),
            outcome: None,
        });
    }

    // Should have removed oldest 10% when limit was hit
    assert!(ctx.conversation_flow.len() <= MAX_FLOW_ITEMS);
}

#[test]
fn test_structured_context_is_empty() {
    let ctx = StructuredContext::new();
    assert!(ctx.is_empty());
    assert_eq!(ctx.total_items(), 0);
}

#[test]
fn a_question_status_defaults_to_open() {
    assert_eq!(QuestionStatus::default(), QuestionStatus::Open);
}
