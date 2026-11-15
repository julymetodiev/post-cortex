// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;
use post_cortex_core::core::context_update::{
    ContextUpdate, EntityRelationship, RelationType, UpdateContent, UpdateType,
};

fn update() -> ContextUpdate {
    ContextUpdate {
        meta: Default::default(),
        id: Uuid::new_v4(),
        timestamp: chrono::Utc::now(),
        update_type: UpdateType::ConceptDefined,
        content: UpdateContent {
            title: "t".into(),
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
        derived_from: Vec::new(),
    }
}

#[test]
fn statistics_do_not_double_count_tiers() {
    let mut session = ActiveSession::new(Uuid::new_v4(), None, None, chrono::Utc::now());
    for _ in 0..3 {
        session.apply_record(update());
    }
    let graph = session.entity_graph_mut();
    for (from, to) in [("a", "b"), ("b", "c")] {
        graph.add_relationship(
            EntityRelationship {
                from_entity: from.into(),
                to_entity: to.into(),
                relation_type: RelationType::RelatedTo,
                context: String::new(),
            },
            chrono::Utc::now(),
        );
    }

    let stats = statistics(Uuid::nil(), &session);
    assert_eq!(stats.hot_updates, 3);
    // Hot entries are also in the incremental log: total is 3, not 6.
    assert_eq!(stats.total_updates, 3);
    assert_eq!(stats.relationship_count, 2);
    assert_eq!(stats.entity_count, 3);
    assert_eq!(stats.duration_minutes, Some(0));
}
