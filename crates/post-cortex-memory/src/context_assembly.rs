// Copyright (c) 2026 Julius ML
//
// Graph-aware context assembly for PCX.
//
// Given a query/hint and a session, assembles the most relevant context by:
// 1. Extracting entities from the query using NER
// 2. Traversing the entity graph to find related entities (typed edges)
// 3. Boosting semantic search results that mention graph-connected entities
// 4. Impact analysis: which entities depend on the query entities
//
// Used by Axon to build LLM context that is structurally relevant,
// not just keyword-similar.

use chrono::Utc;
use post_cortex_core::core::card::{Card, CardScope};
use post_cortex_core::core::context_update::{ContextUpdate, EntityRelationship, RelationType};
use post_cortex_core::graph::entity_graph::SimpleEntityGraph;
use serde::{Deserialize, Serialize};
use std::borrow::Borrow;
use std::collections::{HashMap, HashSet};
use tracing::{debug, info};

/// A single piece of assembled context with its relevance score.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextItem {
    /// The text content
    pub text: String,
    /// Combined relevance score (0.0 - 1.0)
    pub score: f32,
    /// Why this item was included
    pub source: ContextSource,
    /// Entities mentioned in this content
    pub entities: Vec<String>,
    /// Approximate token count
    pub token_estimate: usize,
    /// Stable ID of the ContextUpdate this item was sourced from.
    /// Consumers can use this to track which entries were materialised
    /// into a session's context window and later check their freshness.
    pub entry_id: String,
}

/// How a context item was found
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ContextSource {
    /// Direct semantic search match
    SemanticMatch,
    /// Found via entity graph traversal (entity → related content)
    GraphTraversal {
        /// Entity name that was traversed.
        via_entity: String,
    },
    /// Recent update in the session
    RecentUpdate,
    /// A captured conversation turn (the message stream), found by
    /// semantic search or next to a turn that was
    Conversation,
}

/// Result of graph-aware context assembly
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssembledContext {
    /// Context items sorted by relevance (highest first)
    pub items: Vec<ContextItem>,
    /// Entities relevant to the query, with their graph connections
    pub entity_context: Vec<EntityContext>,
    /// Impact analysis: entities that depend on query entities
    pub impact: Vec<ImpactEntry>,
    /// Total estimated tokens
    pub total_tokens: usize,
    /// The user card and project cards in front of the context (see
    /// [`add_cards`]).
    #[serde(default)]
    pub cards: Vec<Card>,
}

/// An entity and its graph neighborhood relevant to the query
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityContext {
    /// Entity name as stored in the graph.
    pub name: String,
    /// How this entity relates to the query (direct mention, or via graph)
    pub relevance: EntityRelevance,
    /// Typed relationships from the graph
    pub relationships: Vec<EntityRelationship>,
}

/// How an entity relates to the original query.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum EntityRelevance {
    /// Directly mentioned in the query
    DirectMention,
    /// Connected via typed edge in the graph
    GraphNeighbor {
        /// Entity through which this neighbor was discovered.
        via: String,
        /// Type of the graph edge connecting the entities.
        relation: String,
    },
}

/// An entity that would be impacted by changes to a query entity
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImpactEntry {
    /// The entity that depends on the query entity
    pub entity: String,
    /// The query entity it depends on
    pub depends_on: String,
    /// The relationship type
    pub relation_type: RelationType,
    /// How the dependency was found
    pub context: String,
}

/// Tokens [`assemble_context`] keeps free for the entity relationships and
/// impact analysis of `query` (one hop around the entities it names).
pub fn entity_summary_tokens(query: &str, graph: &SimpleEntityGraph) -> usize {
    let query_entities = find_query_entities(query, graph);
    graph_section_tokens(
        &build_entity_context(&query_entities, graph, 1),
        &analyze_impact(&query_entities, graph),
    )
}

/// Tokens the entity-relationship and impact sections take in
/// [`format_for_llm`]'s text.
fn graph_section_tokens(entity_context: &[EntityContext], impact: &[ImpactEntry]) -> usize {
    [entity_section(entity_context), impact_section(impact)]
        .iter()
        .flatten()
        .map(|section| estimate_tokens(section))
        .sum()
}

/// The deduplicated relationship lines of the entity context, if any.
fn entity_section(entity_context: &[EntityContext]) -> Option<String> {
    let mut graph_lines: Vec<String> = entity_context
        .iter()
        .flat_map(|ec| &ec.relationships)
        .map(|rel| {
            format!(
                "  {} --[{:?}]--> {}",
                rel.from_entity, rel.relation_type, rel.to_entity
            )
        })
        .collect();
    if graph_lines.is_empty() {
        return None;
    }
    graph_lines.sort();
    graph_lines.dedup();
    Some(format!("Entity relationships:\n{}", graph_lines.join("\n")))
}

/// The impact-analysis lines, if any.
fn impact_section(impact: &[ImpactEntry]) -> Option<String> {
    if impact.is_empty() {
        return None;
    }
    let impact_lines: Vec<String> = impact
        .iter()
        .map(|i| {
            format!(
                "  {} depends on {} ({:?})",
                i.entity, i.depends_on, i.relation_type
            )
        })
        .collect();
    Some(format!(
        "Impact analysis — these entities depend on what you're working with:\n{}",
        impact_lines.join("\n")
    ))
}

/// Rough token estimate: ~4 chars per token for English
fn estimate_tokens(text: &str) -> usize {
    text.len().div_ceil(4)
}

/// Extract entity names that appear in the query text.
///
/// Uses two strategies:
/// 1. **Exact substring** — entity name appears verbatim in query (case-insensitive)
/// 2. **Token overlap** — split entity names on camelCase/snake_case/hyphens and match
///    individual tokens against query words. Requires ≥50% of entity tokens to match
///    (or all tokens if entity has ≤2 tokens) to avoid false positives.
///
/// Returns entities from the graph that are mentioned in the query, sorted by match quality.
pub fn find_query_entities(query: &str, graph: &SimpleEntityGraph) -> Vec<String> {
    let query_lower = query.to_lowercase();
    let query_tokens: std::collections::HashSet<&str> = query_lower
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|t| t.len() >= 3)
        .collect();

    let mut found: Vec<(&str, usize, bool)> = Vec::new(); // (name, score, is_exact)

    for entity_data in graph.entities.values() {
        let name_lower = entity_data.name.to_lowercase();

        // Skip very short entity names (< 2 chars) to avoid false matches
        if name_lower.len() < 2 {
            continue;
        }

        // Strategy 1: Exact substring match (highest confidence)
        if query_lower.contains(&name_lower) {
            found.push((&entity_data.name, name_lower.len() * 10, true));
            continue;
        }

        // Strategy 2: Token overlap — split entity name into tokens
        let entity_tokens: Vec<String> = split_entity_tokens(&name_lower);
        if entity_tokens.is_empty() {
            continue;
        }

        let matched = entity_tokens
            .iter()
            .filter(|et| {
                if et.len() < 3 {
                    return false;
                }
                query_tokens.iter().any(|qt| {
                    // Exact match or prefix match (stem-like):
                    // "stream" matches "streaming", "chat" matches "chat"
                    let (shorter, longer) = if et.len() <= qt.len() {
                        (et.as_str(), *qt)
                    } else {
                        (*qt, et.as_str())
                    };
                    shorter.len() >= 3 && longer.starts_with(shorter)
                })
            })
            .count();

        if matched == 0 {
            continue;
        }

        // Require sufficient overlap to avoid false positives.
        // For short names (1-2 tokens), require all to match.
        // For medium (3 tokens), at least 1.
        // For longer names, at least 40%.
        let threshold = if entity_tokens.len() <= 2 {
            entity_tokens.len()
        } else {
            1.max((entity_tokens.len() * 2).div_ceil(5)) // ceil(40%)
        };

        if matched >= threshold {
            let score = matched * 5 + name_lower.len();
            found.push((&entity_data.name, score, false));
        }
    }

    // Sort: exact matches first, then by score descending
    found.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| b.1.cmp(&a.1)));
    found
        .into_iter()
        .map(|(name, _, _)| name.to_string())
        .collect()
}

/// Split an entity name into searchable tokens.
///
/// Handles camelCase, PascalCase, snake_case, kebab-case:
///   "ChatRepositoryImpl" → ["chat", "repository", "impl"]
///   "svc-social" → ["svc", "social"]
///   "PG LISTEN/NOTIFY" → ["pg", "listen", "notify"]
fn split_entity_tokens(name: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();

    for c in name.chars() {
        if c == '_' || c == '-' || c == '/' || c == ' ' || c == '.' {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
        } else if c.is_uppercase() && !current.is_empty() {
            // camelCase boundary
            tokens.push(std::mem::take(&mut current));
            current.push(c.to_ascii_lowercase());
        } else {
            current.push(c.to_ascii_lowercase());
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Build entity context: for each query entity, traverse the graph to find
/// related entities and their typed relationships.
pub fn build_entity_context(
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
        Some(rel) if rel.from == from => format!("{:?}", rel.relation_type),
        Some(rel) => format!("{:?} (reverse)", rel.relation_type),
        None => "RelatedTo".to_string(),
    }
}

/// Perform impact analysis: find all entities that depend on any of the query entities.
/// Traverses DependsOn, RequiredBy, and Implements edges in reverse.
pub fn analyze_impact(query_entities: &[String], graph: &SimpleEntityGraph) -> Vec<ImpactEntry> {
    let dependency_types = [
        RelationType::DependsOn,
        RelationType::RequiredBy,
        RelationType::Implements,
    ];

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
                    relation_type: rel.relation_type.clone(),
                    context: rel.context.to_string(),
                });
            }

            // For RequiredBy: if A is required by B, and A is a query entity → B is impacted
            // The edge is: from=A, to=B, type=RequiredBy
            if *rel.relation_type == RelationType::RequiredBy && query_set.contains(rel.from) {
                impacts.push(ImpactEntry {
                    entity: rel.to.to_string(),
                    depends_on: rel.from.to_string(),
                    relation_type: rel.relation_type.clone(),
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

/// Score and boost semantic search results based on entity graph connections.
///
/// Results that mention graph-connected entities get a score boost.
/// This makes structurally related content rank higher than
/// keyword-similar but structurally unrelated content.
pub fn boost_by_graph(
    results: &mut Vec<(String, f32)>, // (text, score)
    entity_context: &[EntityContext],
) {
    // Build a set of all relevant entity names (direct + neighbors)
    let relevant_entities: HashMap<String, f32> = entity_context
        .iter()
        .map(|ec| {
            let boost = match &ec.relevance {
                EntityRelevance::DirectMention => 0.15,
                EntityRelevance::GraphNeighbor { .. } => 0.08,
            };
            (ec.name.to_lowercase(), boost)
        })
        .collect();

    for (text, score) in results.iter_mut() {
        let text_lower = text.to_lowercase();
        let mut total_boost: f32 = 0.0;

        for (entity, boost) in &relevant_entities {
            if text_lower.contains(entity) {
                total_boost += boost;
            }
        }

        // Cap the boost at 0.25 to prevent over-weighting
        *score += total_boost.min(0.25);
        // Clamp to [0, 1]
        *score = score.min(1.0);
    }
}

/// Assemble context from a session's entity graph and context updates.
///
/// This is the main entry point for graph-aware context assembly.
/// It combines entity graph traversal with content scoring to produce
/// a ranked list of context items within a token budget.
///
/// `updates` may be owned updates or shared ones (`&[Arc<ContextUpdate>]`,
/// e.g. a session's `incremental_updates`), so callers need not clone them.
pub fn assemble_context<U: Borrow<ContextUpdate>>(
    query: &str,
    graph: &SimpleEntityGraph,
    updates: &[U],
    token_budget: usize,
) -> AssembledContext {
    info!(
        "Assembling context for query: '{}' (budget: {} tokens)",
        query, token_budget
    );

    // Step 1: Find entities mentioned in the query
    let query_entities = find_query_entities(query, graph);
    debug!("Query entities: {:?}", query_entities);

    // Step 2: Build entity context (graph traversal)
    let entity_context = build_entity_context(&query_entities, graph, 1);
    debug!(
        "Entity context: {} entities (direct + neighbors)",
        entity_context.len()
    );

    // Step 3: Impact analysis
    let impact = analyze_impact(&query_entities, graph);
    if !impact.is_empty() {
        debug!("Impact analysis: {} dependent entities", impact.len());
    }

    // Step 4: Score all updates
    // Lowercase each entity name once, not once per update.
    let entity_names_lower: Vec<(String, &EntityContext)> = entity_context
        .iter()
        .map(|ec| (ec.name.to_lowercase(), ec))
        .collect();
    let now = Utc::now();

    let mut scored_items: Vec<ContextItem> = Vec::with_capacity(updates.len());

    for update in updates {
        let update: &ContextUpdate = update.borrow();
        let text = format!("{}: {}", update.content.title, update.content.description);
        let tokens = estimate_tokens(&text);

        // Base score: recency (newer updates score higher)
        let age_hours = (now - update.timestamp).num_hours().max(0) as f32;
        let recency_score = 1.0 / (1.0 + age_hours / 24.0); // Decays over days

        // Entity match boost
        let text_lower = text.to_lowercase();
        let mut entity_boost: f32 = 0.0;
        let mut matched_entities: Vec<String> = Vec::new();

        for (name_lower, ec) in &entity_names_lower {
            if text_lower.contains(name_lower.as_str()) {
                matched_entities.push(ec.name.clone());
                entity_boost += match &ec.relevance {
                    EntityRelevance::DirectMention => 0.4,
                    EntityRelevance::GraphNeighbor { .. } => 0.2,
                };
            }
        }

        // Importance boost
        let importance_boost = if update.user_marked_important {
            0.2
        } else {
            0.0
        };

        let score = (recency_score * 0.3 + entity_boost + importance_boost).min(1.0);

        // Determine source
        let source = if !matched_entities.is_empty() {
            if query_entities.iter().any(|qe| {
                matched_entities
                    .iter()
                    .any(|me| me.eq_ignore_ascii_case(qe))
            }) {
                ContextSource::SemanticMatch
            } else {
                ContextSource::GraphTraversal {
                    via_entity: matched_entities[0].clone(),
                }
            }
        } else {
            ContextSource::RecentUpdate
        };

        scored_items.push(ContextItem {
            text,
            score,
            source,
            entities: matched_entities,
            token_estimate: tokens,
            entry_id: update.id.to_string(),
        });
    }

    // Step 5: Greedy knapsack — sort by score, pack within budget
    scored_items.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut selected: Vec<ContextItem> = Vec::new();
    let mut used_tokens = 0;

    // Reserve what the entity relationships and impact sections take.
    let entity_summary_tokens = graph_section_tokens(&entity_context, &impact);
    let content_budget = token_budget.saturating_sub(entity_summary_tokens);

    for item in scored_items {
        if used_tokens + item.token_estimate > content_budget {
            // Try to fit — skip items that are too large
            continue;
        }
        used_tokens += item.token_estimate;
        selected.push(item);
    }

    let total_tokens = used_tokens + entity_summary_tokens;
    info!(
        "Assembled {} items ({} tokens), {} entity contexts, {} impact entries",
        selected.len(),
        total_tokens,
        entity_context.len(),
        impact.len()
    );

    AssembledContext {
        items: selected,
        entity_context,
        impact,
        total_tokens,
        cards: Vec::new(),
    }
}

/// Tokens the non-empty `cards` take in front of a context.
pub fn card_tokens(cards: &[Card]) -> usize {
    cards
        .iter()
        .filter(|c| !c.text.trim().is_empty())
        .map(|c| estimate_tokens(&c.text) + 8)
        .sum()
}

/// Put the user and project `cards` in front of `ctx` (empty ones are
/// left out). They always go in; callers keep [`card_tokens`] free in the
/// budget they give the rest.
pub fn add_cards(ctx: &mut AssembledContext, cards: Vec<Card>) {
    let cards: Vec<Card> = cards
        .into_iter()
        .filter(|c| !c.text.trim().is_empty())
        .collect();
    ctx.total_tokens += card_tokens(&cards);
    ctx.cards = cards;
}

/// Add conversation excerpts (see [`ContextSource::Conversation`]) to `ctx`
/// in their order, as far as `token_budget` allows: an excerpt that does not
/// fit is skipped.
pub fn add_excerpts(ctx: &mut AssembledContext, excerpts: Vec<ContextItem>, token_budget: usize) {
    for item in excerpts {
        if ctx.total_tokens + item.token_estimate > token_budget {
            continue;
        }
        ctx.total_tokens += item.token_estimate;
        ctx.items.push(item);
    }
}

/// A conversation excerpt for [`add_excerpts`].
pub fn excerpt(text: String, score: f32, entry_id: String) -> ContextItem {
    ContextItem {
        token_estimate: estimate_tokens(&text),
        text,
        score,
        source: ContextSource::Conversation,
        entities: Vec::new(),
        entry_id,
    }
}

/// Format assembled context as a text block suitable for LLM injection.
pub fn format_for_llm(ctx: &AssembledContext) -> String {
    let mut parts: Vec<String> = Vec::new();

    // Cards first: the standing context of the user and the project.
    for card in &ctx.cards {
        let title = match card.scope {
            CardScope::User => "User card",
            CardScope::Workspace(_) => "Project card",
        };
        parts.push(format!("{title}:\n{}", card.text.trim()));
    }

    // Entity graph summary and impact warnings
    parts.extend(entity_section(&ctx.entity_context));
    parts.extend(impact_section(&ctx.impact));

    // Context items: records, then conversation excerpts
    let (excerpts, records): (Vec<&ContextItem>, Vec<&ContextItem>) = ctx
        .items
        .iter()
        .partition(|item| item.source == ContextSource::Conversation);
    if !records.is_empty() {
        let content_lines: Vec<&str> = records.iter().map(|item| item.text.as_str()).collect();
        parts.push(format!(
            "Relevant context:\n{}",
            content_lines.join("\n---\n")
        ));
    }
    if !excerpts.is_empty() {
        let lines: Vec<&str> = excerpts.iter().map(|item| item.text.as_str()).collect();
        parts.push(format!("Conversation excerpts:\n{}", lines.join("\n")));
    }

    parts.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use post_cortex_core::core::context_update::*;
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
        graph.add_relationship(EntityRelationship {
            from_entity: "Axon".to_string(),
            to_entity: "Post-Cortex".to_string(),
            relation_type: RelationType::DependsOn,
            context: "Axon connects to Post-Cortex".to_string(),
        });
        graph.add_relationship(EntityRelationship {
            from_entity: "Axon".to_string(),
            to_entity: "gRPC".to_string(),
            relation_type: RelationType::DependsOn,
            context: "Axon uses gRPC".to_string(),
        });
        graph.add_relationship(EntityRelationship {
            from_entity: "gRPC".to_string(),
            to_entity: "tonic".to_string(),
            relation_type: RelationType::DependsOn,
            context: "gRPC implemented via tonic".to_string(),
        });
        graph.add_relationship(EntityRelationship {
            from_entity: "Post-Cortex".to_string(),
            to_entity: "RocksDB".to_string(),
            relation_type: RelationType::DependsOn,
            context: "Post-Cortex uses RocksDB for storage".to_string(),
        });
        graph.add_relationship(EntityRelationship {
            from_entity: "Post-Cortex".to_string(),
            to_entity: "Rust".to_string(),
            relation_type: RelationType::DependsOn,
            context: "Post-Cortex built with Rust".to_string(),
        });

        graph
    }

    #[test]
    fn test_find_query_entities() {
        let graph = make_graph();
        let entities = find_query_entities("I'm working on the gRPC service in Axon", &graph);
        assert!(entities.contains(&"gRPC".to_string()));
        assert!(entities.contains(&"Axon".to_string()));
    }

    #[test]
    fn test_build_entity_context_includes_neighbors() {
        let graph = make_graph();
        let query_entities = vec!["gRPC".to_string()];
        let ctx = build_entity_context(&query_entities, &graph, 1);

        let names: Vec<&str> = ctx.iter().map(|ec| ec.name.as_str()).collect();
        // gRPC is direct, tonic and Axon are neighbors
        assert!(names.contains(&"gRPC"));
        assert!(names.contains(&"tonic") || names.contains(&"Axon"));
    }

    #[test]
    fn test_impact_analysis() {
        let graph = make_graph();

        // If RocksDB changes, Post-Cortex is impacted (depends on RocksDB)
        let impact = analyze_impact(&["RocksDB".to_string()], &graph);
        let impacted: Vec<&str> = impact.iter().map(|i| i.entity.as_str()).collect();
        assert!(
            impacted.contains(&"Post-Cortex"),
            "Post-Cortex should be impacted by RocksDB change, got: {:?}",
            impacted
        );

        // If gRPC changes, Axon is impacted
        let impact = analyze_impact(&["gRPC".to_string()], &graph);
        let impacted: Vec<&str> = impact.iter().map(|i| i.entity.as_str()).collect();
        assert!(
            impacted.contains(&"Axon"),
            "Axon should be impacted by gRPC change, got: {:?}",
            impacted
        );
    }

    #[test]
    fn test_assemble_context_with_budget() {
        let graph = make_graph();
        let updates = vec![
            ContextUpdate {
                id: uuid::Uuid::new_v4(),
                update_type: UpdateType::ConceptDefined,
                content: UpdateContent {
                    title: "gRPC Setup".to_string(),
                    description: "Added gRPC service using tonic for Axon communication"
                        .to_string(),
                    details: vec![],
                    examples: vec![],
                    implications: vec![],
                },
                timestamp: Utc::now(),
                related_code: None,
                parent_update: None,
                user_marked_important: false,
                creates_entities: vec![],
                creates_relationships: vec![],
                references_entities: vec![],
                typed_entities: vec![],
                author: None,
            },
            ContextUpdate {
                id: uuid::Uuid::new_v4(),
                update_type: UpdateType::ConceptDefined,
                content: UpdateContent {
                    title: "Unrelated Update".to_string(),
                    description: "Fixed a CSS bug in the landing page".to_string(),
                    details: vec![],
                    examples: vec![],
                    implications: vec![],
                },
                timestamp: Utc::now(),
                related_code: None,
                parent_update: None,
                user_marked_important: false,
                creates_entities: vec![],
                creates_relationships: vec![],
                references_entities: vec![],
                typed_entities: vec![],
                author: None,
            },
        ];

        // Capture IDs so we can verify `entry_id` round-trips below.
        let grpc_id = updates[0].id;
        let css_id = updates[1].id;

        let result = assemble_context("working on gRPC", &graph, &updates, 1000);

        // gRPC-related update should rank higher than CSS bug
        assert!(!result.items.is_empty());
        assert!(result.items[0].text.contains("gRPC"));

        // Entity context should include gRPC and neighbors
        let entity_names: Vec<&str> = result
            .entity_context
            .iter()
            .map(|ec| ec.name.as_str())
            .collect();
        assert!(entity_names.contains(&"gRPC"));

        // Impact: Axon depends on gRPC
        let impacted: Vec<&str> = result.impact.iter().map(|i| i.entity.as_str()).collect();
        assert!(impacted.contains(&"Axon"));

        // Every assembled item should carry the underlying ContextUpdate's
        // ID so consumers (e.g. Axon resume-freshness) can track which
        // entries were materialised into the context window.
        for item in &result.items {
            assert!(!item.entry_id.is_empty(), "entry_id should be populated");
            let id = uuid::Uuid::parse_str(&item.entry_id).expect("entry_id should parse as UUID");
            assert!(
                id == grpc_id || id == css_id,
                "entry_id must match one of the input updates"
            );
        }
    }

    #[test]
    fn test_entity_relationships_cover_both_directions() {
        let graph = make_graph();
        let ctx = build_entity_context(&["gRPC".to_string()], &graph, 1);
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

    #[test]
    fn test_assemble_context_accepts_shared_updates() {
        let graph = make_graph();
        let update = std::sync::Arc::new(ContextUpdate {
            id: uuid::Uuid::new_v4(),
            update_type: UpdateType::ConceptDefined,
            content: UpdateContent {
                title: "RocksDB tuning".to_string(),
                description: "Bloom filters for RocksDB".to_string(),
                details: vec![],
                examples: vec![],
                implications: vec![],
            },
            timestamp: Utc::now(),
            related_code: None,
            parent_update: None,
            user_marked_important: false,
            creates_entities: vec![],
            creates_relationships: vec![],
            references_entities: vec![],
            typed_entities: vec![],
            author: None,
        });
        let shared = vec![std::sync::Arc::clone(&update)];
        let from_shared = assemble_context("rocksdb", &graph, &shared, 1000);
        let from_owned = assemble_context("rocksdb", &graph, &[(*update).clone()], 1000);
        assert_eq!(from_shared.items.len(), 1);
        assert_eq!(from_shared.items[0].text, from_owned.items[0].text);
        assert_eq!(from_shared.items[0].score, from_owned.items[0].score);
        assert_eq!(from_shared.items[0].entities, vec!["RocksDB".to_string()]);
    }

    /// Excerpts fill what the budget leaves, in order, and are formatted as
    /// their own section after the records.
    #[test]
    fn excerpts_fill_the_remaining_budget() {
        let graph = make_graph();
        let mut ctx = assemble_context("rocksdb", &graph, &[] as &[ContextUpdate], 1000);
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
        let updates = vec![ContextUpdate {
            id: uuid::Uuid::new_v4(),
            update_type: UpdateType::ConceptDefined,
            content: UpdateContent {
                title: "RocksDB Migration".to_string(),
                description: "Migrating from sled to RocksDB for better performance".to_string(),
                details: vec![],
                examples: vec![],
                implications: vec![],
            },
            timestamp: Utc::now(),
            related_code: None,
            parent_update: None,
            user_marked_important: false,
            creates_entities: vec![],
            creates_relationships: vec![],
            references_entities: vec![],
            typed_entities: vec![],
            author: None,
        }];

        let result = assemble_context("changing RocksDB", &graph, &updates, 2000);
        let formatted = format_for_llm(&result);

        assert!(formatted.contains("Entity relationships"));
        assert!(formatted.contains("Impact analysis"));
        assert!(formatted.contains("Post-Cortex depends on RocksDB"));
    }
}
