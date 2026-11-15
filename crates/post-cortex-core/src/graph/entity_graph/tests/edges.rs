// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

#[test]
fn test_graph_stats() {
    let graph = create_test_graph();
    let stats = graph.get_graph_stats();

    assert_eq!(stats.node_count, 3);
    assert_eq!(stats.edge_count, 2);
    assert_eq!(stats.entity_count, 3);
    assert!(stats.avg_degree > 0.0);
}

#[test]
fn test_add_relationship_is_idempotent_on_same_triple() {
    let mut graph = create_test_graph();
    let baseline_edges = graph.get_graph_stats().edge_count;

    // Re-add the same (from, to, relation_type) several times with new contexts.
    for ctx in ["first repeat", "second repeat", "third repeat"] {
        graph.add_relationship(
            EntityRelationship {
                from_entity: "rust".to_string(),
                to_entity: "petgraph".to_string(),
                relation_type: RelationType::Implements,
                context: ctx.to_string(),
            },
            Utc::now(),
        );
    }

    assert_eq!(
        graph.get_graph_stats().edge_count,
        baseline_edges,
        "Repeated (from, to, relation_type) must not accumulate parallel edges"
    );

    let all_rels = graph.get_all_relationships();
    let rust_to_petgraph: Vec<_> = all_rels
        .iter()
        .filter(|r| {
            r.from_entity == "rust"
                && r.to_entity == "petgraph"
                && r.relation_type == RelationType::Implements
        })
        .collect();
    assert_eq!(rust_to_petgraph.len(), 1, "Exactly one edge expected");
    assert_eq!(
        rust_to_petgraph[0].context, "third repeat",
        "Context should be updated to the latest value"
    );
}

#[test]
fn test_add_relationship_keeps_distinct_relation_types() {
    let mut graph = SimpleEntityGraph::new();
    graph.add_or_update_entity("a".to_string(), EntityType::Concept, Utc::now(), "");
    graph.add_or_update_entity("b".to_string(), EntityType::Concept, Utc::now(), "");

    for rt in [
        RelationType::DependsOn,
        RelationType::Implements,
        RelationType::RelatedTo,
    ] {
        graph.add_relationship(
            EntityRelationship {
                from_entity: "a".to_string(),
                to_entity: "b".to_string(),
                relation_type: rt,
                context: "".to_string(),
            },
            Utc::now(),
        );
    }

    // Distinct relation types between the same pair are legitimate multi-edges.
    assert_eq!(graph.get_graph_stats().edge_count, 3);
}

#[test]
fn test_relationships_of_and_relation_between() {
    let mut graph = create_test_graph();
    graph.add_relationship(
        EntityRelationship {
            from_entity: "performance".to_string(),
            to_entity: "rust".to_string(),
            relation_type: RelationType::LeadsTo,
            context: "loop back".to_string(),
        },
        Utc::now(),
    );
    graph.add_relationship(
        EntityRelationship {
            from_entity: "rust".to_string(),
            to_entity: "rust".to_string(),
            relation_type: RelationType::RelatedTo,
            context: "self".to_string(),
        },
        Utc::now(),
    );
    assert_eq!(graph.edge_count(), 4);

    let rels = graph.relationships_of("rust");
    // rust->petgraph, rust->rust (once), performance->rust
    assert_eq!(rels.len(), 3);
    assert!(rels.iter().any(|r| r.from == "rust" && r.to == "petgraph"));
    assert!(
        rels.iter()
            .any(|r| r.from == "performance" && r.to == "rust")
    );
    assert_eq!(rels.iter().filter(|r| r.from == r.to).count(), 1);
    assert!(graph.relationships_of("missing").is_empty());

    let between = graph.relation_between("petgraph", "rust");
    assert_eq!(between.len(), 1);
    assert_eq!(between[0].from, "rust");
    assert_eq!(*between[0].relation_type, RelationType::Implements);
    assert_eq!(between[0].context, "Rust implements petgraph");
    assert_eq!(
        between[0].to_relationship().to_entity,
        "petgraph".to_string()
    );
    assert!(graph.relation_between("rust", "performance").len() == 1);
    assert!(graph.relation_between("rust", "nope").is_empty());
}
