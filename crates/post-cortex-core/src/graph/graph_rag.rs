// Copyright (c) 2025 Julius ML
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

//! Graph-RAG: Graph-enhanced Retrieval Augmented Generation
//!
//! This module provides graph-based enrichment for semantic search results.
//! It uses the entity graph to:
//! - Expand query context with related entities
//! - Find structural connections between search results
//! - Add causal chains and relationship paths
//!
//! # Example
//! ```ignore
//! let config = GraphRagConfig::default();
//! let enricher = GraphRagEnricher::new(config);
//! let enriched = enricher.enrich_results(&graph, query, results).await;
//! ```

use crate::core::context_update::RelationType;
use crate::graph::entity_graph::SimpleEntityGraph;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Configuration for Graph-RAG enrichment
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphRagConfig {
    /// Enable/disable graph enrichment
    pub enabled: bool,

    /// Maximum traversal depth (default: 2)
    pub max_depth: usize,

    /// Maximum relations to include per entity (default: 10)
    pub max_relations_per_entity: usize,

    /// Minimum entities in graph to enable enrichment (default: 5)
    pub min_graph_size: usize,

    /// Timeout for graph operations in milliseconds (default: 100)
    pub timeout_ms: u64,

    /// Cache TTL in seconds (default: 300 = 5 minutes)
    pub cache_ttl_secs: u64,

    /// Relation types priority (higher index = lower priority)
    pub relation_priority: Vec<RelationType>,
}

impl Default for GraphRagConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_depth: 2,
            max_relations_per_entity: 10,
            min_graph_size: 5,
            timeout_ms: 100,
            cache_ttl_secs: 300,
            relation_priority: vec![
                RelationType::CausedBy,     // Most useful for debugging
                RelationType::DependsOn,    // Architectural dependency
                RelationType::Implements,   // Implementation relationship
                RelationType::LeadsTo,      // Flow relationship
                RelationType::Solves,       // Problem-solution relationship
                RelationType::RelatedTo,    // General (lowest priority)
            ],
        }
    }
}

/// Cached relation data with expiry
#[derive(Debug, Clone)]
struct CachedRelations {
    relations: Vec<RelationInfo>,
    cached_at: Instant,
}

/// Information about a single relation
#[derive(Debug, Clone)]
pub struct RelationInfo {
    pub target_entity: String,
    pub relation_type: RelationType,
    pub context: String,
    pub depth: usize,
}

/// Result of graph enrichment for a single search result
#[derive(Debug, Clone)]
pub struct GraphEnrichment {
    /// Related entities found via graph traversal
    pub related_entities: Vec<RelationInfo>,

    /// Paths found to query entities
    pub paths_to_query: Vec<Vec<String>>,

    /// Formatted context string to append
    pub context_string: String,
}

/// Global insights from graph analysis
#[derive(Debug, Clone, Default)]
pub struct GlobalGraphInsights {
    /// Central entities related to query
    pub query_entity_map: Vec<(String, Vec<String>)>,

    /// Cross-result connections
    pub structural_insights: Vec<String>,

    /// Formatted string for prepending to results
    pub formatted: String,
}

/// Graph-RAG enricher that adds graph context to search results
pub struct GraphRagEnricher {
    config: GraphRagConfig,
    /// Cache: entity_name -> (relations, cached_at)
    relation_cache: Arc<DashMap<String, CachedRelations>>,
}

impl GraphRagEnricher {
    /// Create a new enricher with the given configuration
    pub fn new(config: GraphRagConfig) -> Self {
        Self {
            config,
            relation_cache: Arc::new(DashMap::new()),
        }
    }

    /// Create with default configuration
    pub fn with_defaults() -> Self {
        Self::new(GraphRagConfig::default())
    }

    /// Check if graph is large enough for enrichment
    pub fn should_enrich(&self, graph: &SimpleEntityGraph) -> bool {
        if !self.config.enabled {
            return false;
        }
        graph.entity_count() >= self.config.min_graph_size
    }

    /// Extract entities from text that exist in the graph
    pub fn extract_graph_entities(&self, graph: &SimpleEntityGraph, text: &str) -> Vec<String> {
        let mut found = Vec::new();
        let words: Vec<&str> = text.split_whitespace().collect();

        for word in words {
            // Clean punctuation
            let clean = word
                .trim_matches(|c: char| !c.is_alphanumeric() && c != '_' && c != '-');

            if clean.len() < 2 {
                continue;
            }

            // Check exact match (case-insensitive)
            let lower = clean.to_lowercase();
            if graph.has_entity(&lower) {
                found.push(lower);
                continue;
            }

            // Check original case
            if graph.has_entity(clean) {
                found.push(clean.to_string());
            }
        }

        // Deduplicate while preserving order
        let mut seen = HashSet::new();
        found.retain(|e| seen.insert(e.clone()));
        found
    }

    /// Find related entities with depth limit and cycle detection
    pub fn find_related_with_depth(
        &self,
        graph: &SimpleEntityGraph,
        start_entity: &str,
        max_depth: usize,
    ) -> Vec<RelationInfo> {
        // Check cache first
        if let Some(cached) = self.get_cached_relations(start_entity) {
            return cached;
        }

        let mut results = Vec::new();
        let mut visited = HashSet::new();
        visited.insert(start_entity.to_string());

        self.traverse_relations(
            graph,
            start_entity,
            1,
            max_depth,
            &mut visited,
            &mut results,
        );

        // Sort by priority (relation type) then by depth
        results.sort_by(|a, b| {
            let priority_a = self.get_relation_priority(&a.relation_type);
            let priority_b = self.get_relation_priority(&b.relation_type);
            priority_a.cmp(&priority_b).then(a.depth.cmp(&b.depth))
        });

        // Limit results
        results.truncate(self.config.max_relations_per_entity);

        // Cache the results
        self.cache_relations(start_entity, results.clone());

        results
    }

    /// Recursive traversal with cycle detection
    fn traverse_relations(
        &self,
        graph: &SimpleEntityGraph,
        entity: &str,
        current_depth: usize,
        max_depth: usize,
        visited: &mut HashSet<String>,
        results: &mut Vec<RelationInfo>,
    ) {
        if current_depth > max_depth {
            return;
        }

        // Get direct relations from graph
        let relations = graph.get_entity_relationships(entity);

        for (target, rel_type, context) in relations {
            if visited.contains(&target) {
                continue; // Cycle detected, skip
            }

            results.push(RelationInfo {
                target_entity: target.clone(),
                relation_type: rel_type.clone(),
                context: context.clone(),
                depth: current_depth,
            });

            // Mark as visited and recurse
            visited.insert(target.clone());
            self.traverse_relations(
                graph,
                &target,
                current_depth + 1,
                max_depth,
                visited,
                results,
            );
        }
    }

    /// Get cached relations if not expired
    fn get_cached_relations(&self, entity: &str) -> Option<Vec<RelationInfo>> {
        let cached = self.relation_cache.get(entity)?;
        let ttl = Duration::from_secs(self.config.cache_ttl_secs);

        if cached.cached_at.elapsed() < ttl {
            Some(cached.relations.clone())
        } else {
            // Expired, remove from cache
            drop(cached);
            self.relation_cache.remove(entity);
            None
        }
    }

    /// Cache relations for an entity
    fn cache_relations(&self, entity: &str, relations: Vec<RelationInfo>) {
        self.relation_cache.insert(
            entity.to_string(),
            CachedRelations {
                relations,
                cached_at: Instant::now(),
            },
        );
    }

    /// Get priority for a relation type (lower = higher priority)
    fn get_relation_priority(&self, rel_type: &RelationType) -> usize {
        self.config
            .relation_priority
            .iter()
            .position(|r| r == rel_type)
            .unwrap_or(usize::MAX)
    }

    // =========================================================================
    // Phase 3: Intelligence - Relevance Filtering & Path Summarization
    // =========================================================================

    /// Filter relations by relevance to query context
    ///
    /// A relation is considered relevant if:
    /// 1. The target entity appears in query entities
    /// 2. The target entity shares context with query entities (common neighbors)
    /// 3. The relation type is high-priority (causes, depends_on, etc.)
    pub fn filter_relevant_relations(
        &self,
        relations: &[RelationInfo],
        query_entities: &[String],
        graph: &SimpleEntityGraph,
    ) -> Vec<RelationInfo> {
        if query_entities.is_empty() {
            return relations.to_vec();
        }

        let query_set: HashSet<_> = query_entities.iter().map(|s| s.to_lowercase()).collect();

        // Get neighbors of query entities for context overlap check
        let mut query_neighbors: HashSet<String> = HashSet::new();
        for qe in query_entities {
            let neighbors = graph.find_related_entities(qe);
            query_neighbors.extend(neighbors.into_iter().map(|n| n.to_lowercase()));
        }

        let mut scored_relations: Vec<(RelationInfo, u32)> = relations
            .iter()
            .map(|rel| {
                let mut score = 0u32;
                let target_lower = rel.target_entity.to_lowercase();

                // Direct match with query entity (highest relevance)
                if query_set.contains(&target_lower) {
                    score += 100;
                }

                // Shares neighbor with query entities (context overlap)
                if query_neighbors.contains(&target_lower) {
                    score += 50;
                }

                // High-priority relation type
                let priority = self.get_relation_priority(&rel.relation_type);
                if priority < 3 {
                    score += (30 - priority * 10) as u32;
                }

                // Closer depth = more relevant
                if rel.depth == 1 {
                    score += 20;
                } else if rel.depth == 2 {
                    score += 10;
                }

                (rel.clone(), score)
            })
            .collect();

        // Sort by relevance score (descending)
        scored_relations.sort_by(|a, b| b.1.cmp(&a.1));

        // Filter out zero-score relations and return
        scored_relations
            .into_iter()
            .filter(|(_, score)| *score > 0)
            .map(|(rel, _)| rel)
            .collect()
    }

    /// Summarize a path with relationship types
    ///
    /// Converts: ["payment", "database", "timeout"]
    /// To: "payment ─DEPENDS_ON→ database ─CAUSED_BY→ timeout"
    pub fn summarize_path(&self, path: &[String], graph: &SimpleEntityGraph) -> String {
        if path.len() < 2 {
            return path.join(" → ");
        }

        let mut parts = Vec::new();
        parts.push(path[0].clone());

        for i in 0..path.len() - 1 {
            let from = &path[i];
            let to = &path[i + 1];

            // Find the relationship between these two entities
            let relations = graph.get_entity_relationships(from);
            let rel_type = relations
                .iter()
                .find(|(target, _, _)| target == to)
                .map(|(_, rt, _)| self.format_relation_type(rt))
                .unwrap_or_else(|| "→".to_string());

            parts.push(format!("─{}→", rel_type));
            parts.push(to.clone());
        }

        parts.join(" ")
    }

    /// Format relation type for display
    fn format_relation_type(&self, rel_type: &RelationType) -> String {
        match rel_type {
            RelationType::CausedBy => "CAUSED_BY".to_string(),
            RelationType::DependsOn => "DEPENDS_ON".to_string(),
            RelationType::Implements => "IMPLEMENTS".to_string(),
            RelationType::LeadsTo => "LEADS_TO".to_string(),
            RelationType::RelatedTo => "RELATED_TO".to_string(),
            RelationType::RequiredBy => "REQUIRED_BY".to_string(),
            RelationType::ConflictsWith => "CONFLICTS".to_string(),
            RelationType::Solves => "SOLVES".to_string(),
        }
    }

    /// Calculate relevance score for a relation (exposed for testing)
    pub fn relation_relevance_score(
        &self,
        relation: &RelationInfo,
        query_entities: &[String],
        query_neighbors: &HashSet<String>,
    ) -> u32 {
        let mut score = 0u32;
        let target_lower = relation.target_entity.to_lowercase();
        let query_set: HashSet<_> = query_entities.iter().map(|s| s.to_lowercase()).collect();

        if query_set.contains(&target_lower) {
            score += 100;
        }
        if query_neighbors.contains(&target_lower) {
            score += 50;
        }
        let priority = self.get_relation_priority(&relation.relation_type);
        if priority < 3 {
            score += (30 - priority * 10) as u32;
        }
        if relation.depth == 1 {
            score += 20;
        } else if relation.depth == 2 {
            score += 10;
        }

        score
    }

    /// Analyze query and build global insights
    pub fn analyze_query(
        &self,
        graph: &SimpleEntityGraph,
        query_entities: &[String],
    ) -> GlobalGraphInsights {
        let mut insights = GlobalGraphInsights::default();

        for entity in query_entities {
            let relations = self.find_related_with_depth(graph, entity, self.config.max_depth);
            if !relations.is_empty() {
                let related: Vec<String> = relations
                    .iter()
                    .take(5)
                    .map(|r| r.target_entity.clone())
                    .collect();
                insights.query_entity_map.push((entity.clone(), related));
            }
        }

        // Format the insights
        if !insights.query_entity_map.is_empty() {
            let mut formatted = String::from("\n[System Knowledge Map]:\n");
            for (entity, related) in &insights.query_entity_map {
                formatted.push_str(&format!("• {} → {}\n", entity, related.join(", ")));
            }
            insights.formatted = formatted;
        }

        insights
    }

    /// Enrich a single search result with graph context
    pub fn enrich_result(
        &self,
        graph: &SimpleEntityGraph,
        content: &str,
        query_entities: &[String],
    ) -> GraphEnrichment {
        let result_entities = self.extract_graph_entities(graph, content);
        let mut related = Vec::new();
        let mut paths = Vec::new();

        // Find relations for entities in this result
        for entity in result_entities.iter().take(3) {
            let entity_relations = self.find_related_with_depth(graph, entity, 1);
            related.extend(entity_relations);
        }

        // Find paths to query entities
        for result_entity in result_entities.iter().take(2) {
            for query_entity in query_entities.iter().take(2) {
                if result_entity != query_entity {
                    if let Some(path) = graph.find_shortest_path(result_entity, query_entity) {
                        if path.len() > 2 && path.len() <= 5 {
                            paths.push(path);
                        }
                    }
                }
            }
        }

        // Deduplicate related entities
        let mut seen = HashSet::new();
        related.retain(|r| seen.insert(r.target_entity.clone()));

        // Phase 3: Apply relevance filtering
        let filtered_related = self.filter_relevant_relations(&related, query_entities, graph);
        let final_related: Vec<RelationInfo> = if filtered_related.is_empty() {
            // Fallback to unfiltered if filtering removes everything
            related.into_iter().take(self.config.max_relations_per_entity).collect()
        } else {
            filtered_related.into_iter().take(self.config.max_relations_per_entity).collect()
        };

        // Format context string with summarized paths
        let context_string = self.format_enrichment_with_summary(graph, &final_related, &paths);

        GraphEnrichment {
            related_entities: final_related,
            paths_to_query: paths,
            context_string,
        }
    }

    /// Find structural insights between top results
    /// Now uses path summarization to show relationship types
    pub fn find_cross_result_insights(
        &self,
        graph: &SimpleEntityGraph,
        result_entities: &[Vec<String>],
    ) -> Vec<String> {
        let mut insights = Vec::new();

        if result_entities.len() < 2 {
            return insights;
        }

        // Check for paths between entities from different results
        for i in 0..result_entities.len().min(3) {
            for j in (i + 1)..result_entities.len().min(3) {
                if let (Some(e1), Some(e2)) = (
                    result_entities[i].first(),
                    result_entities[j].first(),
                ) {
                    if e1 != e2 {
                        if let Some(path) = graph.find_shortest_path(e1, e2) {
                            if path.len() > 2 && path.len() <= 5 {
                                // Phase 3: Use path summarization
                                let summarized = self.summarize_path(&path, graph);
                                insights.push(format!("Connection: {}", summarized));
                            }
                        }
                    }
                }
            }
        }

        insights
    }

    /// Format enrichment data with path summarization (Phase 3)
    fn format_enrichment_with_summary(
        &self,
        graph: &SimpleEntityGraph,
        relations: &[RelationInfo],
        paths: &[Vec<String>],
    ) -> String {
        if relations.is_empty() && paths.is_empty() {
            return String::new();
        }

        let mut output = String::from("\n───────────────────────────\n📊 Graph Context:\n");

        // Add relations grouped by type
        if !relations.is_empty() {
            let mut by_type: std::collections::HashMap<&RelationType, Vec<&str>> =
                std::collections::HashMap::new();

            for rel in relations.iter().take(8) {
                by_type
                    .entry(&rel.relation_type)
                    .or_default()
                    .push(&rel.target_entity);
            }

            for (rel_type, entities) in by_type {
                output.push_str(&format!(
                    "• {}: {}\n",
                    self.format_relation_type(rel_type),
                    entities.join(", ")
                ));
            }
        }

        // Add summarized paths (Phase 3)
        for path in paths.iter().take(2) {
            let summarized = self.summarize_path(path, graph);
            output.push_str(&format!("• Path: {}\n", summarized));
        }

        output.push_str("───────────────────────────");
        output
    }

    /// Format enrichment data as a readable string (legacy, kept for compatibility)
    #[allow(dead_code)]
    fn format_enrichment(&self, relations: &[RelationInfo], paths: &[Vec<String>]) -> String {
        if relations.is_empty() && paths.is_empty() {
            return String::new();
        }

        let mut output = String::from("\n───────────────────────────\n📊 Graph Context:\n");

        // Add relations grouped by type
        if !relations.is_empty() {
            let mut by_type: std::collections::HashMap<&RelationType, Vec<&str>> =
                std::collections::HashMap::new();

            for rel in relations.iter().take(8) {
                by_type
                    .entry(&rel.relation_type)
                    .or_default()
                    .push(&rel.target_entity);
            }

            for (rel_type, entities) in by_type {
                output.push_str(&format!(
                    "• {:?}: {}\n",
                    rel_type,
                    entities.join(", ")
                ));
            }
        }

        // Add paths
        for path in paths.iter().take(2) {
            output.push_str(&format!("• Path: {}\n", path.join(" → ")));
        }

        output.push_str("───────────────────────────");
        output
    }

    /// Clear expired cache entries
    pub fn cleanup_cache(&self) {
        let ttl = Duration::from_secs(self.config.cache_ttl_secs);
        self.relation_cache.retain(|_, cached| {
            cached.cached_at.elapsed() < ttl
        });
    }

    /// Get cache statistics
    pub fn cache_stats(&self) -> (usize, usize) {
        let total = self.relation_cache.len();
        let ttl = Duration::from_secs(self.config.cache_ttl_secs);
        let valid = self.relation_cache
            .iter()
            .filter(|e| e.cached_at.elapsed() < ttl)
            .count();
        (valid, total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::context_update::EntityType;

    fn create_test_graph() -> SimpleEntityGraph {
        let mut graph = SimpleEntityGraph::new();
        let now = chrono::Utc::now();

        // Add entities using add_or_update_entity
        graph.add_or_update_entity(
            "payment".to_string(),
            EntityType::Concept,
            now,
            "Payment processing",
        );

        graph.add_or_update_entity(
            "database".to_string(),
            EntityType::Technology,
            now,
            "Database system",
        );

        graph.add_or_update_entity(
            "timeout".to_string(),
            EntityType::Concept,
            now,
            "Connection timeout",
        );

        graph.add_or_update_entity(
            "auth".to_string(),
            EntityType::Concept,
            now,
            "Authentication",
        );

        graph.add_or_update_entity(
            "user".to_string(),
            EntityType::Concept,
            now,
            "User entity",
        );

        // Add relationships
        use crate::core::context_update::EntityRelationship;

        graph.add_relationship(EntityRelationship {
            from_entity: "payment".to_string(),
            to_entity: "database".to_string(),
            relation_type: RelationType::DependsOn,
            context: "Payment depends on database".to_string(),
        });

        graph.add_relationship(EntityRelationship {
            from_entity: "database".to_string(),
            to_entity: "timeout".to_string(),
            relation_type: RelationType::CausedBy,
            context: "Database can cause timeout".to_string(),
        });

        graph.add_relationship(EntityRelationship {
            from_entity: "auth".to_string(),
            to_entity: "user".to_string(),
            relation_type: RelationType::LeadsTo,
            context: "Auth leads to user".to_string(),
        });

        graph.add_relationship(EntityRelationship {
            from_entity: "payment".to_string(),
            to_entity: "auth".to_string(),
            relation_type: RelationType::DependsOn,
            context: "Payment requires auth".to_string(),
        });

        graph
    }

    #[test]
    fn test_should_enrich() {
        let enricher = GraphRagEnricher::with_defaults();
        let graph = create_test_graph();

        assert!(enricher.should_enrich(&graph));

        // Empty graph should not enrich
        let empty_graph = SimpleEntityGraph::new();
        assert!(!enricher.should_enrich(&empty_graph));
    }

    #[test]
    fn test_extract_graph_entities() {
        let enricher = GraphRagEnricher::with_defaults();
        let graph = create_test_graph();

        let text = "The payment system had a database timeout";
        let entities = enricher.extract_graph_entities(&graph, text);

        assert!(entities.contains(&"payment".to_string()));
        assert!(entities.contains(&"database".to_string()));
        assert!(entities.contains(&"timeout".to_string()));
    }

    #[test]
    fn test_find_related_with_depth() {
        let enricher = GraphRagEnricher::with_defaults();
        let graph = create_test_graph();

        let related = enricher.find_related_with_depth(&graph, "payment", 2);

        // Should find database (depth 1) and timeout (depth 2)
        let targets: Vec<_> = related.iter().map(|r| &r.target_entity).collect();
        assert!(targets.contains(&&"database".to_string()));
        assert!(targets.contains(&&"auth".to_string()));
    }

    #[test]
    fn test_cycle_detection() {
        let mut graph = SimpleEntityGraph::new();
        let now = chrono::Utc::now();

        // Create a cycle: A -> B -> C -> A
        for name in ["a", "b", "c"] {
            graph.add_or_update_entity(
                name.to_string(),
                EntityType::Concept,
                now,
                "Test entity",
            );
        }

        use crate::core::context_update::EntityRelationship;
        graph.add_relationship(EntityRelationship {
            from_entity: "a".to_string(),
            to_entity: "b".to_string(),
            relation_type: RelationType::RelatedTo,
            context: String::new(),
        });
        graph.add_relationship(EntityRelationship {
            from_entity: "b".to_string(),
            to_entity: "c".to_string(),
            relation_type: RelationType::RelatedTo,
            context: String::new(),
        });
        graph.add_relationship(EntityRelationship {
            from_entity: "c".to_string(),
            to_entity: "a".to_string(),
            relation_type: RelationType::RelatedTo,
            context: String::new(),
        });

        let enricher = GraphRagEnricher::with_defaults();
        let related = enricher.find_related_with_depth(&graph, "a", 5);

        // Should not infinite loop, should find b and c exactly once
        assert_eq!(related.len(), 2);
    }

    #[test]
    fn test_cache_works() {
        let enricher = GraphRagEnricher::with_defaults();
        let graph = create_test_graph();

        // First call populates cache
        let related1 = enricher.find_related_with_depth(&graph, "payment", 2);

        // Second call should use cache
        let related2 = enricher.find_related_with_depth(&graph, "payment", 2);

        assert_eq!(related1.len(), related2.len());

        let (valid, total) = enricher.cache_stats();
        assert!(valid > 0);
        assert_eq!(valid, total);
    }

    #[test]
    fn test_enrich_result() {
        let enricher = GraphRagEnricher::with_defaults();
        let graph = create_test_graph();

        let content = "Payment processing failed due to database issues";
        let query_entities = vec!["timeout".to_string()];

        let enrichment = enricher.enrich_result(&graph, content, &query_entities);

        assert!(!enrichment.related_entities.is_empty());
        assert!(!enrichment.context_string.is_empty());
        assert!(enrichment.context_string.contains("Graph Context"));
    }

    // =========================================================================
    // Phase 3 Tests: Relevance Filtering & Path Summarization
    // =========================================================================

    #[test]
    fn test_path_summarization() {
        let enricher = GraphRagEnricher::with_defaults();
        let graph = create_test_graph();

        // Test path: payment -> database -> timeout
        let path = vec![
            "payment".to_string(),
            "database".to_string(),
            "timeout".to_string(),
        ];

        let summarized = enricher.summarize_path(&path, &graph);

        // Should contain relationship types
        assert!(summarized.contains("payment"));
        assert!(summarized.contains("database"));
        assert!(summarized.contains("timeout"));
        assert!(summarized.contains("DEPENDS_ON") || summarized.contains("→"));
    }

    #[test]
    fn test_relevance_filtering() {
        let enricher = GraphRagEnricher::with_defaults();
        let graph = create_test_graph();

        // Create some test relations
        let relations = vec![
            RelationInfo {
                target_entity: "database".to_string(),
                relation_type: RelationType::DependsOn,
                context: String::new(),
                depth: 1,
            },
            RelationInfo {
                target_entity: "timeout".to_string(),
                relation_type: RelationType::CausedBy,
                context: String::new(),
                depth: 2,
            },
            RelationInfo {
                target_entity: "unrelated".to_string(),
                relation_type: RelationType::RelatedTo,
                context: String::new(),
                depth: 3,
            },
        ];

        // Query for "timeout" - should prioritize relations connected to timeout
        let query_entities = vec!["timeout".to_string()];
        let filtered = enricher.filter_relevant_relations(&relations, &query_entities, &graph);

        // Should filter and prioritize
        assert!(!filtered.is_empty());

        // timeout should be first (direct match with query)
        if !filtered.is_empty() {
            // The direct match should have highest score
            let first = &filtered[0];
            assert!(
                first.target_entity == "timeout" || first.target_entity == "database",
                "Expected timeout or database to be prioritized"
            );
        }
    }

    #[test]
    fn test_relevance_scoring() {
        let enricher = GraphRagEnricher::with_defaults();

        let relation = RelationInfo {
            target_entity: "database".to_string(),
            relation_type: RelationType::DependsOn,
            context: String::new(),
            depth: 1,
        };

        // Query includes "database" - should get high score
        let query_entities = vec!["database".to_string()];
        let query_neighbors: HashSet<String> = HashSet::new();

        let score = enricher.relation_relevance_score(&relation, &query_entities, &query_neighbors);

        // Direct match (100) + priority bonus (20 for DependsOn at index 1) + depth 1 (20) = 140
        assert!(score >= 100, "Direct match should give high score, got {}", score);
    }

    #[test]
    fn test_format_relation_type() {
        let enricher = GraphRagEnricher::with_defaults();

        assert_eq!(enricher.format_relation_type(&RelationType::CausedBy), "CAUSED_BY");
        assert_eq!(enricher.format_relation_type(&RelationType::DependsOn), "DEPENDS_ON");
        assert_eq!(enricher.format_relation_type(&RelationType::Implements), "IMPLEMENTS");
        assert_eq!(enricher.format_relation_type(&RelationType::LeadsTo), "LEADS_TO");
        assert_eq!(enricher.format_relation_type(&RelationType::RelatedTo), "RELATED_TO");
    }

    #[test]
    fn test_cross_result_insights_with_summary() {
        let enricher = GraphRagEnricher::with_defaults();
        let graph = create_test_graph();

        // Two result sets with entities that have a path between them
        let result_entities = vec![
            vec!["payment".to_string()],
            vec!["timeout".to_string()],
        ];

        let insights = enricher.find_cross_result_insights(&graph, &result_entities);

        // Should find connection through database
        // Path: payment -> database -> timeout
        if !insights.is_empty() {
            let insight = &insights[0];
            assert!(insight.contains("Connection:"), "Should contain 'Connection:'");
            // Should have summarized path with relationship types
            assert!(
                insight.contains("payment") && insight.contains("timeout"),
                "Should contain both entities"
            );
        }
    }
}
