// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The query's entities in the graph: their neighbourhood and what
//! depends on them.

use crate::context_assembly::format::graph_section_tokens;
use crate::context_assembly::{EntityContext, EntityRelevance, ImpactEntry};
use post_cortex_core::core::context_update::{EntityRelationship, RelationType};
use post_cortex_core::graph::entity_graph::{SimpleEntityGraph, entities_in};
use std::collections::HashSet;

/// The graph around a query: the entities it names with their one-hop
/// neighbourhood, what depends on them, and the tokens both take.
#[derive(Debug, Clone, Default)]
pub struct GraphContext {
    /// The named entities first, then their neighbours.
    pub entity_context: Vec<EntityContext>,
    /// The entities that depend on the named ones.
    pub impact: Vec<ImpactEntry>,
    /// Tokens the entity relationships and impact sections take.
    pub tokens: usize,
}

/// The graph around the entities `query` names, within `max_tokens`: when
/// it does not fit, the neighbours go first (the last found first), then
/// the impact entries, then the named entities, so a query that names a
/// hub cannot take the room of the records.
pub fn graph_context(query: &str, graph: &SimpleEntityGraph, max_tokens: usize) -> GraphContext {
    let query_entities = entities_in(query, graph);
    let mut entity_context = build_entity_context(&query_entities, graph, 1);
    let mut impact = analyze_impact(&query_entities, graph);
    let mut tokens = graph_section_tokens(&entity_context, &impact);
    while tokens > max_tokens {
        let neighbour = entity_context
            .iter()
            .rposition(|ec| ec.relevance != EntityRelevance::DirectMention);
        match neighbour {
            Some(at) => {
                entity_context.remove(at);
            }
            None if !impact.is_empty() => {
                impact.pop();
            }
            None => {
                entity_context.pop();
            }
        }
        tokens = graph_section_tokens(&entity_context, &impact);
    }
    GraphContext {
        entity_context,
        impact,
        tokens,
    }
}

/// Build entity context: for each query entity, traverse the graph to find
/// related entities and their typed relationships.
fn build_entity_context(
    query_entities: &[String],
    graph: &SimpleEntityGraph,
    max_depth: usize,
) -> Vec<EntityContext> {
    let mut result: Vec<EntityContext> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    // First: direct mentions
    for entity in query_entities {
        if seen.contains(entity) {
            continue;
        }
        seen.insert(entity.clone());

        let rels = get_entity_relationships(entity, graph);
        result.push(EntityContext {
            name: entity.clone(),
            relevance: EntityRelevance::DirectMention,
            relationships: rels,
        });
    }

    // Then: graph neighbors (depth 1 and optionally 2)
    for depth in 0..max_depth {
        let current_entities: Vec<String> = result
            .iter()
            .filter(|ec| {
                if depth == 0 {
                    ec.relevance == EntityRelevance::DirectMention
                } else {
                    true
                }
            })
            .map(|ec| ec.name.clone())
            .collect();

        for entity in &current_entities {
            let neighbors = graph.find_related_entities(entity);
            for neighbor in neighbors {
                if seen.contains(&neighbor) {
                    continue;
                }
                seen.insert(neighbor.clone());

                // Find the relationship type between entity and neighbor
                let rel_desc = get_relationship_description(entity, &neighbor, graph);
                let rels = get_entity_relationships(&neighbor, graph);

                result.push(EntityContext {
                    name: neighbor.clone(),
                    relevance: EntityRelevance::GraphNeighbor {
                        via: entity.clone(),
                        relation: rel_desc,
                    },
                    relationships: rels,
                });
            }
        }
    }

    result
}

/// Get all relationships for an entity (outgoing, then incoming). O(degree).
fn get_entity_relationships(entity: &str, graph: &SimpleEntityGraph) -> Vec<EntityRelationship> {
    graph
        .relationships_of(entity)
        .iter()
        .map(|r| r.to_relationship())
        .collect()
}

/// Get a human-readable description of the relationship between two entities
fn get_relationship_description(from: &str, to: &str, graph: &SimpleEntityGraph) -> String {
    match graph.relation_between(from, to).first() {
        Some(rel) if rel.from == from => rel.relation_type.stored_name().to_string(),
        Some(rel) => format!("{} (reverse)", rel.relation_type.stored_name()),
        None => "RelatedTo".to_string(),
    }
}

/// Perform impact analysis: find all entities that depend on any of the query entities.
/// `B depends_on A` and `B implements A` make B a dependent of A; `A
/// required_by B` does too (B needs A), so a `required_by` edge is read in
/// its own direction, never the other way.
pub fn analyze_impact(query_entities: &[String], graph: &SimpleEntityGraph) -> Vec<ImpactEntry> {
    let dependency_types = [RelationType::DependsOn, RelationType::Implements];

    let mut impacts: Vec<ImpactEntry> = Vec::new();
    let query_set: HashSet<&str> = query_entities.iter().map(String::as_str).collect();

    // Every impacting edge touches a query entity, so walking each query
    // entity's own edges (O(degree)) finds them all.
    for query_entity in query_entities {
        for rel in graph.relationships_of(query_entity) {
            // Skip self-references
            if rel.from == rel.to {
                continue;
            }

            // For DependsOn: if B depends on A, and A is a query entity → B is impacted
            // The edge is: from=B, to=A, type=DependsOn
            if dependency_types.contains(rel.relation_type) && query_set.contains(rel.to) {
                impacts.push(ImpactEntry {
                    entity: rel.from.to_string(),
                    depends_on: rel.to.to_string(),
                    relation_type: *rel.relation_type,
                    context: rel.context.to_string(),
                });
            }

            // For RequiredBy: if A is required by B, and A is a query entity → B is impacted
            // The edge is: from=A, to=B, type=RequiredBy
            if *rel.relation_type == RelationType::RequiredBy && query_set.contains(rel.from) {
                impacts.push(ImpactEntry {
                    entity: rel.to.to_string(),
                    depends_on: rel.from.to_string(),
                    relation_type: *rel.relation_type,
                    context: rel.context.to_string(),
                });
            }
        }
    }

    // Deduplicate by (entity, depends_on)
    let mut seen: HashSet<(String, String)> = HashSet::new();
    impacts.retain(|i| seen.insert((i.entity.clone(), i.depends_on.clone())));
    impacts
}
