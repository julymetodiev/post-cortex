// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

#[test]
fn test_petgraph_find_related_entities() {
    let graph = create_test_graph();

    let related = graph.find_related_entities("rust");
    assert!(related.contains(&"petgraph".to_string()));

    let related = graph.find_related_entities("petgraph");
    assert!(related.contains(&"rust".to_string()));
    assert!(related.contains(&"performance".to_string()));
}

#[test]
fn test_shortest_path() {
    let graph = create_test_graph();

    let path = graph.find_shortest_path("rust", "performance");
    assert!(path.is_some());

    let path = graph.find_shortest_path("rust", "nonexistent");
    assert!(path.is_none());
}

#[test]
fn test_trace_relationships_petgraph() {
    let graph = create_test_graph();

    let trace = graph.trace_entity_relationships("rust", 2);
    assert!(!trace.is_empty());

    // Should find path rust -> petgraph -> performance
    let has_rust_to_petgraph = trace
        .iter()
        .any(|(from, _, to)| from == "rust" && to == "petgraph");
    assert!(has_rust_to_petgraph);
}

#[test]
fn test_entity_network_petgraph() {
    let graph = create_test_graph();

    let network = graph.get_entity_network("petgraph", 2);
    assert_eq!(network.center, "petgraph");
    assert!(network.entities.contains_key("petgraph"));
    assert!(network.entities.contains_key("rust"));
    assert!(network.entities.contains_key("performance"));
    assert!(!network.relationships.is_empty());
}

#[test]
fn test_shortest_path_returns_full_path() {
    // Create a longer chain: a -> b -> c -> d
    let mut graph = SimpleEntityGraph::new();

    for entity in ["a", "b", "c", "d"] {
        graph.add_or_update_entity(entity.to_string(), EntityType::Concept, Utc::now(), "");
    }

    graph.add_relationship(
        EntityRelationship {
            from_entity: "a".to_string(),
            to_entity: "b".to_string(),
            relation_type: RelationType::LeadsTo,
            context: "".to_string(),
        },
        Utc::now(),
    );
    graph.add_relationship(
        EntityRelationship {
            from_entity: "b".to_string(),
            to_entity: "c".to_string(),
            relation_type: RelationType::LeadsTo,
            context: "".to_string(),
        },
        Utc::now(),
    );
    graph.add_relationship(
        EntityRelationship {
            from_entity: "c".to_string(),
            to_entity: "d".to_string(),
            relation_type: RelationType::LeadsTo,
            context: "".to_string(),
        },
        Utc::now(),
    );

    let path = graph.find_shortest_path("a", "d");
    assert!(path.is_some(), "Path should exist from a to d");

    let path = path.unwrap();
    // Should return full path: [a, b, c, d], not just [a, d]
    assert_eq!(path.len(), 4, "Path should have 4 nodes: a -> b -> c -> d");
    assert_eq!(path[0], "a");
    assert_eq!(path[1], "b");
    assert_eq!(path[2], "c");
    assert_eq!(path[3], "d");
}

#[test]
fn test_shortest_path_finds_shortest() {
    // Create graph with two paths: a -> b -> d (length 2) and a -> c -> e -> d (length 3)
    let mut graph = SimpleEntityGraph::new();

    for entity in ["a", "b", "c", "d", "e"] {
        graph.add_or_update_entity(entity.to_string(), EntityType::Concept, Utc::now(), "");
    }

    // Short path: a -> b -> d
    graph.add_relationship(
        EntityRelationship {
            from_entity: "a".to_string(),
            to_entity: "b".to_string(),
            relation_type: RelationType::LeadsTo,
            context: "".to_string(),
        },
        Utc::now(),
    );
    graph.add_relationship(
        EntityRelationship {
            from_entity: "b".to_string(),
            to_entity: "d".to_string(),
            relation_type: RelationType::LeadsTo,
            context: "".to_string(),
        },
        Utc::now(),
    );

    // Long path: a -> c -> e -> d
    graph.add_relationship(
        EntityRelationship {
            from_entity: "a".to_string(),
            to_entity: "c".to_string(),
            relation_type: RelationType::LeadsTo,
            context: "".to_string(),
        },
        Utc::now(),
    );
    graph.add_relationship(
        EntityRelationship {
            from_entity: "c".to_string(),
            to_entity: "e".to_string(),
            relation_type: RelationType::LeadsTo,
            context: "".to_string(),
        },
        Utc::now(),
    );
    graph.add_relationship(
        EntityRelationship {
            from_entity: "e".to_string(),
            to_entity: "d".to_string(),
            relation_type: RelationType::LeadsTo,
            context: "".to_string(),
        },
        Utc::now(),
    );

    let path = graph.find_shortest_path("a", "d").unwrap();
    // Should find the shorter path (length 3: a, b, d)
    assert_eq!(path.len(), 3, "Should find shortest path with 3 nodes");
    assert_eq!(path[0], "a");
    assert_eq!(path[2], "d");
}

#[test]
fn test_find_related_entities_returns_sorted() {
    let mut graph = SimpleEntityGraph::new();

    // Add entities
    for entity in ["center", "zebra", "apple", "mango", "banana"] {
        graph.add_or_update_entity(entity.to_string(), EntityType::Concept, Utc::now(), "");
    }

    // Connect all to center (in non-alphabetical order)
    for entity in ["zebra", "apple", "mango", "banana"] {
        graph.add_relationship(
            EntityRelationship {
                from_entity: "center".to_string(),
                to_entity: entity.to_string(),
                relation_type: RelationType::RelatedTo,
                context: "".to_string(),
            },
            Utc::now(),
        );
    }

    let related = graph.find_related_entities("center");

    // Verify results are sorted alphabetically
    assert_eq!(related.len(), 4);
    assert_eq!(related[0], "apple");
    assert_eq!(related[1], "banana");
    assert_eq!(related[2], "mango");
    assert_eq!(related[3], "zebra");
}

#[test]
fn bounded_network_matches_full_network_when_limits_are_loose() {
    let graph = hub_graph(5);
    let full = graph.get_entity_network("hub", 2);
    let bounded = graph.get_entity_network_bounded("hub", 2, usize::MAX, usize::MAX);

    assert_eq!(
        bounded.entities.keys().collect::<Vec<_>>(),
        full.entities.keys().collect::<Vec<_>>()
    );
    let key = |r: &EntityRelationship| (r.from_entity.clone(), r.to_entity.clone());
    let mut a: Vec<_> = full.relationships.iter().map(key).collect();
    let mut b: Vec<_> = bounded.relationships.iter().map(key).collect();
    a.sort();
    b.sort();
    assert_eq!(a, b);
    // Direction is kept: the cycle edge points back at the hub.
    assert!(b.contains(&("s0".into(), "hub".into())));
}

#[test]
fn bounded_network_keeps_the_closest_entities_and_honours_limits() {
    let graph = hub_graph(10);
    let network = graph.get_entity_network_bounded("hub", 2, 4, 3);

    assert_eq!(network.entities.len(), 4);
    assert!(network.entities.contains_key("hub"));
    // Breadth-first: direct neighbours fill the budget before any leaf.
    assert!(network.entities.keys().all(|k| !k.starts_with("leaf")));
    assert_eq!(network.relationships.len(), 3);
    for r in &network.relationships {
        assert!(network.entities.contains_key(&r.from_entity));
        assert!(network.entities.contains_key(&r.to_entity));
    }

    assert!(
        graph
            .get_entity_network_bounded("missing", 2, 10, 10)
            .entities
            .is_empty()
    );
    assert!(
        graph
            .get_entity_network_bounded("hub", 2, 0, 10)
            .entities
            .is_empty()
    );
}

#[test]
fn test_entity_network_no_duplicate_relationships() {
    let mut graph = SimpleEntityGraph::new();

    // Create a small network
    for entity in ["a", "b", "c"] {
        graph.add_or_update_entity(entity.to_string(), EntityType::Concept, Utc::now(), "");
    }

    // Create bidirectional relationships (could cause duplicates)
    graph.add_relationship(
        EntityRelationship {
            from_entity: "a".to_string(),
            to_entity: "b".to_string(),
            relation_type: RelationType::RelatedTo,
            context: "".to_string(),
        },
        Utc::now(),
    );
    graph.add_relationship(
        EntityRelationship {
            from_entity: "b".to_string(),
            to_entity: "c".to_string(),
            relation_type: RelationType::RelatedTo,
            context: "".to_string(),
        },
        Utc::now(),
    );
    graph.add_relationship(
        EntityRelationship {
            from_entity: "c".to_string(),
            to_entity: "a".to_string(),
            relation_type: RelationType::RelatedTo,
            context: "".to_string(),
        },
        Utc::now(),
    );

    let network = graph.get_entity_network("a", 3);

    // Count occurrences of each relationship
    let mut rel_counts: std::collections::HashMap<(String, String, RelationType), usize> =
        std::collections::HashMap::new();
    for rel in &network.relationships {
        let key = (
            rel.from_entity.clone(),
            rel.to_entity.clone(),
            rel.relation_type,
        );
        *rel_counts.entry(key).or_insert(0) += 1;
    }

    // Verify no duplicates (all counts should be 1)
    for ((from, to, _), count) in rel_counts {
        assert_eq!(
            count, 1,
            "Relationship {}->{} should appear exactly once, found {}",
            from, to, count
        );
    }
}
