// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Scores over the graph: centrality, importance analysis and recency.

use crate::core::context_update::EntityData;
use crate::graph::entity_graph::{SimpleEntityGraph, top_k_by};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

impl SimpleEntityGraph {
    /// Get the `limit` most important entities (highest
    /// [`centrality`](Self::centrality) first, ties by name). Top-k
    /// selection.
    pub fn get_most_important_entities(&self, limit: usize) -> Vec<&EntityData> {
        let centrality = self.centrality();
        let score = |e: &EntityData| centrality.get(&e.name).copied().unwrap_or(0.0);
        top_k_by(self.entities.values_unordered().collect(), limit, |a, b| {
            score(b)
                .total_cmp(&score(a))
                .then_with(|| a.name.cmp(&b.name))
        })
    }

    /// Get the `limit` most recently mentioned entities (newest first, ties
    /// by name). Top-k selection.
    pub fn get_recently_mentioned_entities(&self, limit: usize) -> Vec<&EntityData> {
        top_k_by(self.entities.values_unordered().collect(), limit, |a, b| {
            b.last_mentioned
                .cmp(&a.last_mentioned)
                .then_with(|| a.name.cmp(&b.name))
        })
    }

    /// Importance of every entity in (0, 1]: centrality in the weighted
    /// relation graph, personalised by mentions and faded by staleness
    /// (see [`crate::graph::analytics::importance`]).
    pub fn centrality(&self) -> BTreeMap<String, f32> {
        crate::graph::analytics::importance(self)
    }

    /// Analyze entity importance with graph metrics
    ///
    /// Sorted by [`centrality`](Self::centrality) (highest first), ties by
    /// name.
    pub fn analyze_entity_importance(&self) -> Vec<EntityAnalysis> {
        self.analyze_entity_importance_top(usize::MAX)
    }

    /// The `limit` highest-ranked entries of
    /// [`analyze_entity_importance`](Self::analyze_entity_importance),
    /// computed with top-k selection; only the survivors are materialised
    /// (names cloned).
    pub fn analyze_entity_importance_top(&self, limit: usize) -> Vec<EntityAnalysis> {
        struct Scored<'a> {
            score: f32,
            name: &'a String,
            entity: &'a EntityData,
            relationship_count: usize,
            update_references: usize,
        }

        let centrality = self.centrality();
        let mut scored = Vec::with_capacity(self.entities.len());
        // Any order: the top-k below breaks ties by name.
        for (name, entity) in self.entities.iter_unordered() {
            let relationship_count = self.relationships_of(name).len();

            let update_references = self
                .entity_mentions
                .get(name)
                .map(|mentions| mentions.len())
                .unwrap_or(0);

            scored.push(Scored {
                score: centrality.get(name).copied().unwrap_or(0.0),
                name,
                entity,
                relationship_count,
                update_references,
            });
        }

        // Highest score first (NaN-safe); ties by name, which matches the
        // previous stable sort over the BTreeMap's name order.
        top_k_by(scored, limit, |a, b| {
            b.score.total_cmp(&a.score).then_with(|| a.name.cmp(b.name))
        })
        .into_iter()
        .map(|s| EntityAnalysis {
            entity_name: s.name.clone(),
            importance_score: s.score,
            mention_count: s.entity.mention_count,
            relationship_count: s.relationship_count,
            update_references: s.update_references,
            first_seen: s.entity.first_mentioned,
            last_seen: s.entity.last_mentioned,
        })
        .collect()
    }

    /// How many entries of
    /// [`analyze_entity_importance`](Self::analyze_entity_importance) have
    /// an `importance_score` of at least `min`, without building them.
    pub fn count_entities_with_importance_at_least(&self, min: f32) -> usize {
        self.centrality().values().filter(|s| **s >= min).count()
    }
}

/// Entity analysis structure - unchanged for compatibility
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EntityAnalysis {
    /// Name of the analyzed entity
    pub entity_name: String,
    /// Combined importance score including graph centrality
    pub importance_score: f32,
    /// Total number of times the entity was mentioned
    pub mention_count: u32,
    /// Number of graph edges connected to this entity
    pub relationship_count: usize,
    /// Number of context updates referencing this entity
    pub update_references: usize,
    /// Timestamp when the entity was first seen
    pub first_seen: DateTime<Utc>,
    /// Timestamp when the entity was last referenced
    pub last_seen: DateTime<Utc>,
}
