// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Tests of the entity graph, one file per concern.

mod codec;
mod edges;
mod matching;
mod mentions;
mod ranking;
mod traversal;

use super::*;
use crate::core::context_update::{ContextUpdate, RelationType};

pub(super) fn create_test_graph() -> SimpleEntityGraph {
    let mut graph = SimpleEntityGraph::new();

    // Add some entities
    graph.add_or_update_entity(
        "rust".to_string(),
        EntityType::Technology,
        Utc::now(),
        "Systems programming language",
    );

    graph.add_or_update_entity(
        "petgraph".to_string(),
        EntityType::Technology,
        Utc::now(),
        "Graph data structure library",
    );

    graph.add_or_update_entity(
        "performance".to_string(),
        EntityType::Concept,
        Utc::now(),
        "System performance optimization",
    );

    // Add relationships
    graph.add_relationship(
        EntityRelationship {
            from_entity: "rust".to_string(),
            to_entity: "petgraph".to_string(),
            relation_type: RelationType::Implements,
            context: "Rust implements petgraph".to_string(),
        },
        Utc::now(),
    );

    graph.add_relationship(
        EntityRelationship {
            from_entity: "petgraph".to_string(),
            to_entity: "performance".to_string(),
            relation_type: RelationType::Implements,
            context: "Petgraph improves performance".to_string(),
        },
        Utc::now(),
    );

    graph
}

/// `hub -> s0..s{n}` plus `s{i} -> leaf{i}` and a cycle `s0 -> hub`.
pub(super) fn hub_graph(spokes: usize) -> SimpleEntityGraph {
    let mut graph = SimpleEntityGraph::new();
    let rel = |from: &str, to: &str| EntityRelationship {
        from_entity: from.to_string(),
        to_entity: to.to_string(),
        relation_type: RelationType::RelatedTo,
        context: format!("{from}->{to}"),
    };
    graph.add_or_update_entity("hub".into(), EntityType::Concept, Utc::now(), "");
    for i in 0..spokes {
        for name in [format!("s{i}"), format!("leaf{i}")] {
            graph.add_or_update_entity(name, EntityType::Concept, Utc::now(), "");
        }
        graph.add_relationship(rel("hub", &format!("s{i}")), Utc::now());
        graph.add_relationship(rel(&format!("s{i}"), &format!("leaf{i}")), Utc::now());
    }
    graph.add_relationship(rel("s0", "hub"), Utc::now());
    graph
}

pub(super) fn at(secs: i64) -> DateTime<Utc> {
    use chrono::TimeZone;
    Utc.timestamp_opt(1_750_000_000 + secs, 0).unwrap()
}

pub(super) fn record(secs: i64, entities: &[&str], relations: &[(&str, &str)]) -> ContextUpdate {
    ContextUpdate {
        meta: Default::default(),
        id: uuid::Uuid::new_v4(),
        timestamp: at(secs),
        update_type: crate::core::context_update::UpdateType::ConceptDefined,
        content: crate::core::context_update::UpdateContent {
            title: format!("record at {secs}"),
            description: String::new(),
            details: vec![],
            examples: vec![],
            implications: vec![],
        },
        related_code: None,
        parent_update: None,
        user_marked_important: false,
        creates_entities: entities.iter().map(|e| e.to_string()).collect(),
        creates_relationships: relations
            .iter()
            .map(|(a, b)| EntityRelationship {
                from_entity: a.to_string(),
                to_entity: b.to_string(),
                relation_type: RelationType::DependsOn,
                context: String::new(),
            })
            .collect(),
        references_entities: vec![],
        typed_entities: vec![],
        author: None,
        derived_from: Vec::new(),
    }
}
