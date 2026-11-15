// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;
use chrono::Utc;
use post_cortex_core::core::context_update::{EntityRelationship, EntityType, RelationType};
use post_cortex_core::graph::entity_graph::SimpleEntityGraph;
fn make_graph() -> SimpleEntityGraph {
    let mut graph = SimpleEntityGraph::new();
    let now = Utc::now();

    // Add entities
    graph.add_or_update_entity("Axon".into(), EntityType::Technology, now, "");
    graph.add_or_update_entity("Post-Cortex".into(), EntityType::Technology, now, "");
    graph.add_or_update_entity("gRPC".into(), EntityType::Technology, now, "");
    graph.add_or_update_entity("tonic".into(), EntityType::Technology, now, "");
    graph.add_or_update_entity("RocksDB".into(), EntityType::Technology, now, "");
    graph.add_or_update_entity("Rust".into(), EntityType::Technology, now, "");

    // Add typed relationships
    graph.add_relationship(
        EntityRelationship {
            from_entity: "Axon".to_string(),
            to_entity: "Post-Cortex".to_string(),
            relation_type: RelationType::DependsOn,
            context: "Axon connects to Post-Cortex".to_string(),
        },
        chrono::Utc::now(),
    );
    graph.add_relationship(
        EntityRelationship {
            from_entity: "Axon".to_string(),
            to_entity: "gRPC".to_string(),
            relation_type: RelationType::DependsOn,
            context: "Axon uses gRPC".to_string(),
        },
        chrono::Utc::now(),
    );
    graph.add_relationship(
        EntityRelationship {
            from_entity: "gRPC".to_string(),
            to_entity: "tonic".to_string(),
            relation_type: RelationType::DependsOn,
            context: "gRPC implemented via tonic".to_string(),
        },
        chrono::Utc::now(),
    );
    graph.add_relationship(
        EntityRelationship {
            from_entity: "Post-Cortex".to_string(),
            to_entity: "RocksDB".to_string(),
            relation_type: RelationType::DependsOn,
            context: "Post-Cortex uses RocksDB for storage".to_string(),
        },
        chrono::Utc::now(),
    );
    graph.add_relationship(
        EntityRelationship {
            from_entity: "Post-Cortex".to_string(),
            to_entity: "Rust".to_string(),
            relation_type: RelationType::DependsOn,
            context: "Post-Cortex built with Rust".to_string(),
        },
        chrono::Utc::now(),
    );

    graph
}

#[test]
fn the_graph_context_has_the_named_entity_its_neighbours_and_what_depends_on_it() {
    let graph = make_graph();
    let around = graph_context("working on gRPC", &graph, usize::MAX);
    let names: Vec<&str> = around
        .entity_context
        .iter()
        .map(|ec| ec.name.as_str())
        .collect();
    assert_eq!(names[0], "gRPC", "the named entity first");
    assert!(
        names.contains(&"tonic") && names.contains(&"Axon"),
        "{names:?}"
    );
    let impacted: Vec<&str> = around.impact.iter().map(|i| i.entity.as_str()).collect();
    assert_eq!(impacted, ["Axon"], "Axon depends on gRPC");

    let around = graph_context("changing RocksDB", &graph, usize::MAX);
    let impacted: Vec<&str> = around.impact.iter().map(|i| i.entity.as_str()).collect();
    assert_eq!(impacted, ["Post-Cortex"], "Post-Cortex depends on RocksDB");
}

#[test]
fn test_entity_relationships_cover_both_directions() {
    let graph = make_graph();
    let ctx = graph_context("gRPC", &graph, usize::MAX).entity_context;
    let grpc = ctx.iter().find(|ec| ec.name == "gRPC").unwrap();
    let mut edges: Vec<(&str, &str)> = grpc
        .relationships
        .iter()
        .map(|r| (r.from_entity.as_str(), r.to_entity.as_str()))
        .collect();
    edges.sort_unstable();
    assert_eq!(edges, vec![("Axon", "gRPC"), ("gRPC", "tonic")]);

    let axon = ctx.iter().find(|ec| ec.name == "Axon").unwrap();
    assert_eq!(
        axon.relevance,
        EntityRelevance::GraphNeighbor {
            via: "gRPC".to_string(),
            relation: "DependsOn (reverse)".to_string(),
        }
    );
    let tonic = ctx.iter().find(|ec| ec.name == "tonic").unwrap();
    assert_eq!(
        tonic.relevance,
        EntityRelevance::GraphNeighbor {
            via: "gRPC".to_string(),
            relation: "DependsOn".to_string(),
        }
    );
}

/// A graph context that does not fit drops the neighbours first, then the
/// impact, and keeps the named entity as long as it fits.
#[test]
fn the_graph_context_stays_within_its_tokens() {
    let graph = make_graph();
    let whole = graph_context("Post-Cortex", &graph, usize::MAX);
    assert!(whole.entity_context.len() > 1);
    let named_only = GraphContext {
        entity_context: whole.entity_context[..1].to_vec(),
        impact: Vec::new(),
        tokens: 0,
    };
    let cap = format_for_llm(&assemble_context(named_only, Vec::new(), usize::MAX))
        .len()
        .div_ceil(4);
    let trimmed = graph_context("Post-Cortex", &graph, cap);
    assert!(trimmed.tokens <= cap, "{} > {cap}", trimmed.tokens);
    let names: Vec<&str> = trimmed
        .entity_context
        .iter()
        .map(|ec| ec.name.as_str())
        .collect();
    assert_eq!(names, ["Post-Cortex"]);
    assert!(trimmed.impact.is_empty());
    assert_eq!(graph_context("Post-Cortex", &graph, 0).tokens, 0);
}

/// The records go in as search ranked them, after the graph, as far as the
/// budget allows; each names the graph's entities its text mentions.
#[test]
fn records_fill_what_the_graph_leaves_in_their_order() {
    let graph = make_graph();
    let around = graph_context("working on gRPC", &graph, 500);
    let graph_tokens = around.tokens;
    let budget = graph_tokens + 60;
    let records = vec![
        record("gRPC Setup: tonic for Axon".to_string(), 0.9, "a".into()),
        record("x".repeat(400), 0.8, "b".into()),
        record("CSS bug on the landing page".to_string(), 0.4, "c".into()),
    ];
    let ctx = assemble_context(around, records, budget);
    let ids: Vec<&str> = ctx.items.iter().map(|i| i.entry_id.as_str()).collect();
    assert_eq!(ids, ["a", "c"], "the long one does not fit");
    assert!(ctx.total_tokens <= budget);
    let mut named = ctx.items[0].entities.clone();
    named.sort();
    assert_eq!(named, ["Axon", "gRPC", "tonic"]);
    assert!(ctx.items[1].entities.is_empty());
    assert!(
        ctx.items
            .iter()
            .all(|i| i.source == ContextSource::SemanticMatch)
    );
}

/// Excerpts fill what the budget leaves, in order, and are formatted as
/// their own section after the records.
#[test]
fn excerpts_fill_the_remaining_budget() {
    let graph = make_graph();
    let mut ctx = assemble_context(graph_context("rocksdb", &graph, 500), Vec::new(), 1000);
    let base = ctx.total_tokens;
    let turn = |text: &str| excerpt(text.to_string(), 0.5, format!("s/{text}"));
    let long = "x".repeat(4 * (1000 - base) + 40);
    add_excerpts(
        &mut ctx,
        vec![
            turn("user: why rocksdb?"),
            turn(&long),
            turn("assistant: speed"),
        ],
        1000,
    );
    let texts: Vec<&str> = ctx.items.iter().map(|i| i.text.as_str()).collect();
    assert_eq!(
        texts,
        ["user: why rocksdb?", "assistant: speed"],
        "the long one does not fit"
    );
    assert!(ctx.total_tokens <= 1000);
    let formatted = format_for_llm(&ctx);
    assert!(
        formatted.contains("Conversation excerpts:\nuser: why rocksdb?\nassistant: speed"),
        "{formatted}"
    );
    assert!(!formatted.contains("Relevant context:"));
}

#[test]
fn test_format_for_llm() {
    let graph = make_graph();
    let records = vec![record(
        "RocksDB Migration: from sled to RocksDB for better performance".to_string(),
        0.9,
        uuid::Uuid::new_v4().to_string(),
    )];
    let result = assemble_context(
        graph_context("changing RocksDB", &graph, 1000),
        records,
        2000,
    );
    let formatted = format_for_llm(&result);

    assert!(formatted.contains("Entity relationships"));
    assert!(formatted.contains("Impact analysis"));
    assert!(formatted.contains("Post-Cortex depends on RocksDB"));
    assert!(formatted.contains("Relevant context:\nRocksDB Migration"));
}
