// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Graph-aware context assembly: the pure part of the Assembly capability
//! (`capabilities::assembly`).
//!
//! For a query it packs into a token budget:
//! 1. the user and project cards (always);
//! 2. the graph's entities the query names
//!    ([`entities_in`](post_cortex_core::graph::entity_graph::entities_in)),
//!    their one-hop neighbourhood (typed edges) and what depends on them,
//!    within half of what the cards leave ([`graph_context`]);
//! 3. the records the Search capability ranked for the query, best first;
//! 4. conversation excerpts, when asked for.

mod assemble;
mod format;
mod graph;
#[cfg(test)]
mod tests;

pub use assemble::{add_cards, add_excerpts, assemble_context, card_tokens, excerpt, record};
pub use format::format_for_llm;
pub use graph::{GraphContext, analyze_impact, graph_context};

use post_cortex_core::core::card::Card;
use post_cortex_core::core::context_update::{EntityRelationship, RelationType};
use serde::{Deserialize, Serialize};

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
    /// A record (or a workspace's document) the Search capability ranked
    /// for the query: by meaning, by words and, in a session, by its graph
    SemanticMatch,
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
