// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

#[test]
fn test_graph_serialization_persistence() {
    // Create a complex graph with multiple entities and relationships
    let mut original_graph = SimpleEntityGraph::new();

    // Add entities
    original_graph.add_or_update_entity(
        "rust".to_string(),
        EntityType::Technology,
        Utc::now(),
        "Systems programming language",
    );
    original_graph.add_or_update_entity(
        "petgraph".to_string(),
        EntityType::Technology,
        Utc::now(),
        "Graph library",
    );
    original_graph.add_or_update_entity(
        "performance".to_string(),
        EntityType::Concept,
        Utc::now(),
        "Performance optimization",
    );
    original_graph.add_or_update_entity(
        "lock-free".to_string(),
        EntityType::Concept,
        Utc::now(),
        "Lock-free data structures",
    );

    // Add relationships
    original_graph.add_relationship(
        EntityRelationship {
            from_entity: "rust".to_string(),
            to_entity: "petgraph".to_string(),
            relation_type: RelationType::Implements,
            context: "Rust implements petgraph".to_string(),
        },
        Utc::now(),
    );

    original_graph.add_relationship(
        EntityRelationship {
            from_entity: "petgraph".to_string(),
            to_entity: "performance".to_string(),
            relation_type: RelationType::LeadsTo,
            context: "Petgraph improves performance".to_string(),
        },
        Utc::now(),
    );

    original_graph.add_relationship(
        EntityRelationship {
            from_entity: "lock-free".to_string(),
            to_entity: "performance".to_string(),
            relation_type: RelationType::LeadsTo,
            context: "Lock-free improves performance".to_string(),
        },
        Utc::now(),
    );

    original_graph.add_relationship(
        EntityRelationship {
            from_entity: "rust".to_string(),
            to_entity: "lock-free".to_string(),
            relation_type: RelationType::Implements,
            context: "Rust supports lock-free".to_string(),
        },
        Utc::now(),
    );

    // Get original state
    let original_stats = original_graph.get_graph_stats();
    let original_rust_related = original_graph.find_related_entities("rust");
    let original_petgraph_related = original_graph.find_related_entities("petgraph");
    let original_trace = original_graph.trace_entity_relationships("rust", 3);
    let original_network = original_graph.get_entity_network("rust", 2);

    // Serialize
    let serialized = serde_json::to_string(&original_graph).expect("Serialization failed");

    // Deserialize
    let deserialized_graph: SimpleEntityGraph =
        serde_json::from_str(&serialized).expect("Deserialization failed");

    // Verify graph stats match
    let deserialized_stats = deserialized_graph.get_graph_stats();
    assert_eq!(
        original_stats.node_count, deserialized_stats.node_count,
        "Node count mismatch"
    );
    assert_eq!(
        original_stats.edge_count, deserialized_stats.edge_count,
        "Edge count mismatch"
    );
    assert_eq!(
        original_stats.entity_count, deserialized_stats.entity_count,
        "Entity count mismatch"
    );

    // Verify relationships are preserved
    let deserialized_rust_related = deserialized_graph.find_related_entities("rust");
    assert_eq!(
        original_rust_related.len(),
        deserialized_rust_related.len(),
        "Rust relationships count mismatch"
    );
    assert!(
        deserialized_rust_related.contains(&"petgraph".to_string()),
        "Missing rust->petgraph relationship"
    );
    assert!(
        deserialized_rust_related.contains(&"lock-free".to_string()),
        "Missing rust->lock-free relationship"
    );

    let deserialized_petgraph_related = deserialized_graph.find_related_entities("petgraph");
    assert_eq!(
        original_petgraph_related.len(),
        deserialized_petgraph_related.len(),
        "Petgraph relationships count mismatch"
    );

    // Verify graph traversal works
    let deserialized_trace = deserialized_graph.trace_entity_relationships("rust", 3);
    assert_eq!(
        original_trace.len(),
        deserialized_trace.len(),
        "Trace length mismatch"
    );

    // Verify entity network works
    let deserialized_network = deserialized_graph.get_entity_network("rust", 2);
    assert_eq!(
        original_network.entities.len(),
        deserialized_network.entities.len(),
        "Network entities count mismatch"
    );
    assert_eq!(
        original_network.relationships.len(),
        deserialized_network.relationships.len(),
        "Network relationships count mismatch"
    );

    // Every entity came back, with its relations.
    assert_eq!(deserialized_graph.entity_count(), 4);
    for entity_name in ["rust", "petgraph", "performance", "lock-free"] {
        assert!(
            deserialized_graph.has_entity(entity_name),
            "missing entity {entity_name}"
        );
    }
    assert_eq!(
        serde_json::to_string(&deserialized_graph).unwrap(),
        serde_json::to_string(&original_graph).unwrap(),
        "a round trip writes the same bytes"
    );

    // Verify specific relationship queries work
    let mut rust_implements: Vec<String> = deserialized_graph
        .relationships_of("rust")
        .into_iter()
        .filter(|r| r.from == "rust" && *r.relation_type == RelationType::Implements)
        .map(|r| r.to.to_string())
        .collect();
    rust_implements.sort();
    assert_eq!(
        rust_implements.len(),
        2,
        "Should have 2 'Implements' relationships"
    );
    assert!(rust_implements.contains(&"petgraph".to_string()));
    assert!(rust_implements.contains(&"lock-free".to_string()));

    // Verify shortest path still works
    let path = deserialized_graph.find_shortest_path("rust", "performance");
    assert!(path.is_some(), "Should find path from rust to performance");

    println!("✓ Graph serialization/deserialization preserves all relationships and functionality");
}

#[test]
fn test_empty_graph_serialization() {
    let original = SimpleEntityGraph::new();
    let serialized = serde_json::to_string(&original).expect("Serialization failed");
    let deserialized: SimpleEntityGraph =
        serde_json::from_str(&serialized).expect("Deserialization failed");

    assert_eq!(deserialized.get_graph_stats().node_count, 0);
    assert_eq!(deserialized.get_graph_stats().edge_count, 0);
}

#[test]
fn test_graph_with_orphan_entities() {
    // Test graph with entities that have no relationships
    let mut graph = SimpleEntityGraph::new();

    graph.add_or_update_entity(
        "orphan1".to_string(),
        EntityType::Concept,
        Utc::now(),
        "Orphan entity",
    );
    graph.add_or_update_entity(
        "orphan2".to_string(),
        EntityType::Concept,
        Utc::now(),
        "Another orphan",
    );
    graph.add_or_update_entity(
        "connected1".to_string(),
        EntityType::Concept,
        Utc::now(),
        "Connected entity",
    );
    graph.add_or_update_entity(
        "connected2".to_string(),
        EntityType::Concept,
        Utc::now(),
        "Another connected",
    );

    graph.add_relationship(
        EntityRelationship {
            from_entity: "connected1".to_string(),
            to_entity: "connected2".to_string(),
            relation_type: RelationType::RelatedTo,
            context: "test".to_string(),
        },
        Utc::now(),
    );

    let serialized = serde_json::to_string(&graph).expect("Serialization failed");
    let deserialized: SimpleEntityGraph =
        serde_json::from_str(&serialized).expect("Deserialization failed");

    // All entities should be preserved
    assert_eq!(deserialized.entities.len(), 4);
    assert_eq!(deserialized.get_graph_stats().node_count, 4);
    assert_eq!(deserialized.get_graph_stats().edge_count, 1);

    // Verify orphan entities are accessible
    assert!(deserialized.entities.contains_key("orphan1"));
    assert!(deserialized.entities.contains_key("orphan2"));

    // Verify connected entities maintain their relationship
    let connected1_related = deserialized.find_related_entities("connected1");
    assert!(connected1_related.contains(&"connected2".to_string()));
}

#[test]
fn test_relation_type_ordering() {
    // Verify RelationType implements Ord correctly
    assert!(RelationType::RequiredBy < RelationType::LeadsTo);
    assert!(RelationType::LeadsTo < RelationType::RelatedTo);
    assert!(RelationType::RelatedTo < RelationType::ConflictsWith);
    assert!(RelationType::ConflictsWith < RelationType::DependsOn);
    assert!(RelationType::DependsOn < RelationType::Implements);
    assert!(RelationType::Implements < RelationType::CausedBy);
    assert!(RelationType::CausedBy < RelationType::Solves);

    // Test that sorting works
    let mut types = [
        RelationType::Solves,
        RelationType::RequiredBy,
        RelationType::LeadsTo,
    ];
    types.sort();
    assert_eq!(types[0], RelationType::RequiredBy);
    assert_eq!(types[1], RelationType::LeadsTo);
    assert_eq!(types[2], RelationType::Solves);
}

#[test]
fn test_serialization_determinism() {
    // Verify that serializing the same graph twice produces identical JSON
    // With BTreeMap, the entire JSON should be deterministic
    let mut graph = SimpleEntityGraph::new();

    // Add entities in non-alphabetical order
    for entity in ["zebra", "apple", "mango", "banana"] {
        graph.add_or_update_entity(entity.to_string(), EntityType::Concept, Utc::now(), "");
    }

    // Add relationships
    graph.add_relationship(
        EntityRelationship {
            from_entity: "zebra".to_string(),
            to_entity: "apple".to_string(),
            relation_type: RelationType::RelatedTo,
            context: "".to_string(),
        },
        Utc::now(),
    );
    graph.add_relationship(
        EntityRelationship {
            from_entity: "apple".to_string(),
            to_entity: "mango".to_string(),
            relation_type: RelationType::LeadsTo,
            context: "".to_string(),
        },
        Utc::now(),
    );

    // Serialize twice - should be identical with BTreeMap
    let json1 = serde_json::to_string(&graph).unwrap();
    let json2 = serde_json::to_string(&graph).unwrap();
    assert_eq!(json1, json2, "Serialization should be deterministic");

    // Parse and verify structure
    let parsed: serde_json::Value = serde_json::from_str(&json1).unwrap();

    // Verify entities are sorted alphabetically (BTreeMap)
    let entities_keys: Vec<&str> = parsed["entities"]
        .as_object()
        .unwrap()
        .keys()
        .map(|s| s.as_str())
        .collect();
    assert_eq!(entities_keys, vec!["apple", "banana", "mango", "zebra"]);

    // Verify graph nodes are sorted alphabetically
    let nodes = parsed["graph"]["nodes"].as_array().unwrap();
    assert_eq!(nodes[0], "apple");
    assert_eq!(nodes[1], "banana");
    assert_eq!(nodes[2], "mango");
    assert_eq!(nodes[3], "zebra");

    // Verify edges are sorted (by from_entity, then to_entity)
    let edges = parsed["graph"]["edges"].as_array().unwrap();
    assert_eq!(edges[0][0], "apple"); // apple -> mango comes before zebra -> apple
    assert_eq!(edges[1][0], "zebra");

    // Deserialize and re-serialize - should produce identical JSON
    let deserialized: SimpleEntityGraph = serde_json::from_str(&json1).unwrap();
    let json3 = serde_json::to_string(&deserialized).unwrap();
    assert_eq!(
        json1, json3,
        "Re-serialization should produce identical JSON"
    );

    // Verify functionality preserved
    assert_eq!(deserialized.entities.len(), 4);
    assert_eq!(deserialized.get_graph_stats().edge_count, 2);

    // Verify relationships work after deserialization
    let related = deserialized.find_related_entities("apple");
    assert!(related.contains(&"zebra".to_string()));
    assert!(related.contains(&"mango".to_string()));
}
