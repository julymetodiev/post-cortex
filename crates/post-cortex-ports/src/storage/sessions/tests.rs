// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;
use post_cortex_core::core::context_update::{UpdateContent, UpdateType};

fn update(id: u128) -> ContextUpdate {
    ContextUpdate {
        id: Uuid::from_u128(id),
        timestamp: chrono::DateTime::UNIX_EPOCH,
        update_type: UpdateType::DecisionMade,
        content: UpdateContent {
            title: format!("t{id}"),
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
        meta: Default::default(),
    }
}

/// A later request's row or removal of a record wins over an earlier one;
/// a later (un)vectorization of an id replaces an earlier one.
#[test]
fn merged_changes_keep_the_latest_of_each_record_and_id() {
    let (a, b, c) = (Uuid::from_u128(1), Uuid::from_u128(2), Uuid::from_u128(3));
    let mut changes = SessionChanges {
        updates: vec![update(1), update(2)],
        vectorized: vec![a, b],
        ..Default::default()
    };
    changes.merge(SessionChanges {
        updates: vec![update(3)],
        removed: vec![a],
        unvectorized: vec![a],
        ..Default::default()
    });
    changes.merge(SessionChanges {
        whole_graph: true,
        ..Default::default()
    });
    let ids: Vec<Uuid> = changes.updates.iter().map(|u| u.id).collect();
    assert_eq!(ids, [b, c]);
    assert_eq!(changes.removed, [a]);
    assert_eq!(changes.vectorized, [b]);
    assert_eq!(changes.unvectorized, [a]);
    assert!(changes.whole_graph);
    assert!(!changes.is_empty());
    assert!(SessionChanges::default().is_empty());
}
