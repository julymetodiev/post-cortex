// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

#[test]
fn test_enhanced_importance_scoring() {
    let mut graph = create_test_graph();

    // Add more relationships to petgraph to increase its importance
    graph.add_or_update_entity(
        "optimization".to_string(),
        EntityType::Concept,
        Utc::now(),
        "Code optimization",
    );

    graph.add_relationship(
        EntityRelationship {
            from_entity: "petgraph".to_string(),
            to_entity: "optimization".to_string(),
            relation_type: RelationType::LeadsTo,
            context: "Petgraph leads to optimization".to_string(),
        },
        Utc::now(),
    );

    let analysis = graph.analyze_entity_importance();

    // Petgraph should have higher importance due to more connections
    let petgraph_analysis = analysis
        .iter()
        .find(|a| a.entity_name == "petgraph")
        .unwrap();

    let rust_analysis = analysis.iter().find(|a| a.entity_name == "rust").unwrap();

    assert!(petgraph_analysis.importance_score >= rust_analysis.importance_score);
    assert_eq!(petgraph_analysis.relationship_count, 3); // Connected to rust, performance, optimization
}

#[test]
fn importance_count_matches_the_full_analysis() {
    let graph = hub_graph(6);
    let full = graph.analyze_entity_importance();
    for min in [0.0, 0.5, 1.05, 1.2, 10.0] {
        let expected = full.iter().filter(|e| e.importance_score >= min).count();
        assert_eq!(
            graph.count_entities_with_importance_at_least(min),
            expected,
            "min {min}"
        );
    }
}

#[test]
fn test_analyze_entity_importance_capacity_preallocation() {
    // Test that analyze_entity_importance works correctly with many entities
    let mut graph = SimpleEntityGraph::new();

    // Add many entities to test capacity pre-allocation
    for i in 0..100 {
        graph.add_or_update_entity(format!("entity_{}", i), EntityType::Concept, Utc::now(), "");
    }

    // Add some relationships
    for i in 0..50 {
        graph.add_relationship(
            EntityRelationship {
                from_entity: format!("entity_{}", i),
                to_entity: format!("entity_{}", i + 50),
                relation_type: RelationType::RelatedTo,
                context: "".to_string(),
            },
            Utc::now(),
        );
    }

    let analysis = graph.analyze_entity_importance();

    // Should have all 100 entities analyzed
    assert_eq!(analysis.len(), 100);

    // Verify it's sorted by importance (descending)
    for i in 1..analysis.len() {
        assert!(
            analysis[i - 1].importance_score >= analysis[i].importance_score,
            "Analysis should be sorted by importance descending"
        );
    }
}

#[test]
fn test_importance_ordering_is_nan_safe_and_top_k() {
    let mut graph = SimpleEntityGraph::new();
    let now = Utc::now();
    for (i, name) in ["a", "b", "c", "d", "e"].iter().enumerate() {
        graph.add_or_update_entity(name.to_string(), EntityType::Concept, now, "");
        // Without relations, importance follows the mentions.
        graph.entities.get_mut(*name).unwrap().mention_count = i as u32 + 1;
    }
    // The stored score no longer ranks; a NaN there must not panic
    // (release profile is panic = "abort").
    graph.entities.get_mut("c").unwrap().importance_score = f32::NAN;

    let analysis = graph.analyze_entity_importance();
    assert_eq!(analysis.len(), 5);
    assert!(analysis.iter().all(|a| a.importance_score.is_finite()));

    let top: Vec<&str> = graph
        .get_most_important_entities(2)
        .iter()
        .map(|e| e.name.as_str())
        .collect();
    assert_eq!(top, vec!["e", "d"]);

    let all: Vec<&str> = graph
        .get_most_important_entities(100)
        .iter()
        .map(|e| e.name.as_str())
        .collect();
    assert_eq!(all, vec!["e", "d", "c", "b", "a"]);
    assert!(graph.get_most_important_entities(0).is_empty());
    assert_eq!(graph.get_recently_mentioned_entities(3).len(), 3);
}
