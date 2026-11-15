// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;
use crate::core::context_update::{EntityRelationship, EntityType, RelationType};

fn graph(edges: &[(&str, &str, RelationType)]) -> SimpleEntityGraph {
    let mut g = SimpleEntityGraph::new();
    let now = Utc::now();
    for (a, b, _) in edges {
        g.add_or_update_entity(a.to_string(), EntityType::Concept, now, "");
        g.add_or_update_entity(b.to_string(), EntityType::Concept, now, "");
    }
    for (a, b, r) in edges {
        g.add_relationship(
            EntityRelationship {
                from_entity: a.to_string(),
                to_entity: b.to_string(),
                relation_type: *r,
                context: String::new(),
            },
            chrono::Utc::now(),
        );
    }
    g
}

#[test]
fn hub_is_most_important() {
    let g = graph(&[
        ("hub", "a", RelationType::DependsOn),
        ("hub", "b", RelationType::DependsOn),
        ("hub", "c", RelationType::DependsOn),
        ("c", "d", RelationType::RelatedTo),
    ]);
    let scores = importance(&g);
    assert_eq!(scores["hub"], 1.0);
    assert!(scores.values().all(|s| *s > 0.0 && *s <= 1.0));
    assert!(scores["a"] < scores["hub"]);
    assert!(scores["d"] < scores["c"]);
}

#[test]
fn stale_entities_fade() {
    let mut g = graph(&[("x", "y", RelationType::DependsOn)]);
    g.entities.get_mut("y").unwrap().last_mentioned -= chrono::Duration::days(90);
    let scores = importance(&g);
    // Symmetric pair: same centrality, y faded to 75%.
    assert!(
        (scores["y"] / scores["x"] - 0.75).abs() < 1e-3,
        "{scores:?}"
    );
}

#[test]
fn two_cliques_are_two_communities() {
    use RelationType::DependsOn as D;
    let g = graph(&[
        ("a1", "a2", D),
        ("a2", "a3", D),
        ("a1", "a3", D),
        ("b1", "b2", D),
        ("b2", "b3", D),
        ("b1", "b3", D),
        ("a3", "b1", RelationType::RelatedTo),
    ]);
    let c = communities(&g);
    assert_eq!(c["a1"], c["a2"]);
    assert_eq!(c["a1"], c["a3"]);
    assert_eq!(c["b1"], c["b2"]);
    assert_eq!(c["b1"], c["b3"]);
    assert_ne!(c["a1"], c["b1"]);
    assert_eq!(communities(&g), c, "deterministic");
}

#[test]
fn isolated_entities_have_no_community() {
    let mut g = graph(&[("a", "b", RelationType::DependsOn)]);
    g.add_or_update_entity("lonely".into(), EntityType::Concept, Utc::now(), "");
    let c = communities(&g);
    assert!(!c.contains_key("lonely"));
    assert_eq!(c.len(), 2);
}

/// The levels sit on the (0, 1] scale of `importance`: the most central
/// entity is critical (on the old 2.0 / 1.5 / 1.0 scale every entity was
/// at most low), a peripheral one lower.
#[test]
fn the_top_entity_is_critical() {
    use crate::summary::ImportanceLevel;
    let g = graph(&[
        ("hub", "a", RelationType::DependsOn),
        ("hub", "b", RelationType::DependsOn),
        ("hub", "c", RelationType::DependsOn),
        ("c", "d", RelationType::RelatedTo),
    ]);
    let scores = importance(&g);
    assert_eq!(importance_level(scores["hub"]), ImportanceLevel::Critical);
    assert_ne!(importance_level(scores["d"]), ImportanceLevel::Critical);
    assert_eq!(importance_level(0.05), ImportanceLevel::Minimal);
}
