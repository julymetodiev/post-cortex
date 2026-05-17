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
//! Semantic Query Engine
//!
//! This module provides advanced semantic search capabilities across sessions,
//! enabling intelligent context discovery and cross-session relationship analysis.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{debug, info};
use uuid::Uuid;

use crate::core::content_vectorizer::{ContentVectorizer, SemanticSearchResult};

/// Related experience from other sessions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelatedExperience {
    pub session_id: Uuid,
    pub similarity_score: f32,
    pub relevance_score: f32,
    pub content_snippet: String,
    pub content_type: String,
    pub timestamp: DateTime<Utc>,
    pub context: String,
}

/// Enhanced context with semantic relationships
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnhancedContext {
    pub primary_content: Vec<SemanticSearchResult>,
    pub related_experiences: Vec<RelatedExperience>,
    pub confidence_score: f32,
    pub semantic_themes: Vec<String>,
    pub suggested_actions: Vec<String>,
}

/// Semantic insights about a query or topic
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticInsights {
    pub dominant_themes: Vec<String>,
    pub related_concepts: Vec<String>,
    pub user_patterns: Vec<String>,
    pub decision_history: Vec<String>,
    pub problem_patterns: Vec<String>,
    pub knowledge_gaps: Vec<String>,
}

/// Configuration for semantic query engine
#[derive(Debug, Clone)]
pub struct SemanticQueryConfig {
    pub max_related_experiences: usize,
    pub similarity_threshold: f32,
    pub cross_session_enabled: bool,
    pub insight_confidence_threshold: f32,
    pub max_context_results: usize,
    /// Temporal decay factor for recency bias (0.0 = disabled)
    pub recency_bias: f32,
}

impl Default for SemanticQueryConfig {
    fn default() -> Self {
        Self {
            max_related_experiences: 10,
            similarity_threshold: 0.7,
            cross_session_enabled: true,
            insight_confidence_threshold: 0.6,
            max_context_results: 20,
            recency_bias: 0.0, // Disabled by default for backward compatibility
        }
    }
}

/// Main semantic query engine
pub struct SemanticQueryEngine {
    vectorizer: ContentVectorizer,
    #[allow(dead_code)]
    config: SemanticQueryConfig,
}

impl SemanticQueryEngine {
    /// Create a new semantic query engine
    pub fn new(
        vectorizer: ContentVectorizer,
        config: SemanticQueryConfig,
    ) -> Self {
        Self {
            vectorizer,
            config,
        }
    }

    /// Perform semantic search across all sessions
    pub async fn semantic_search_global(
        &self,
        query: &str,
        limit: Option<usize>,
        date_range: Option<(chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)>,
        recency_bias: Option<f32>,
    ) -> Result<Vec<SemanticSearchResult>> {
        let search_limit = limit.unwrap_or(self.config.max_context_results);

        info!("Performing global semantic search for: '{}'", query);
        if let Some((start, end)) = date_range {
            info!("  Date range filter: {} to {}", start, end);
        }

        let options = crate::core::content_vectorizer::SearchOptions {
            limit: Some(search_limit),
            date_range,
            recency_bias,
        };

        let results = self
            .vectorizer
            .semantic_search(query, search_limit, None, options)
            .await
            .context("Failed to perform semantic search")?;

        // Filter by similarity threshold
        let filtered_results: Vec<_> = results
            .into_iter()
            .filter(|r| r.similarity_score >= self.config.similarity_threshold)
            .collect();

        debug!(
            "Global semantic search returned {} results (filtered from threshold {})",
            filtered_results.len(),
            self.config.similarity_threshold
        );

        Ok(filtered_results)
    }

    /// Search within a specific session
    pub async fn semantic_search_session(
        &self,
        session_id: Uuid,
        query: &str,
        limit: Option<usize>,
        date_range: Option<(chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)>,
        recency_bias: Option<f32>,
    ) -> Result<Vec<SemanticSearchResult>> {
        let search_limit = limit.unwrap_or(self.config.max_context_results);

        debug!(
            "Performing session-specific semantic search in {} for: '{}'",
            session_id, query
        );
        if let Some((start, end)) = date_range {
            debug!("  Date range filter: {} to {}", start, end);
        }

        let options = crate::core::content_vectorizer::SearchOptions {
            limit: Some(search_limit),
            date_range,
            recency_bias,
        };

        let results = self
            .vectorizer
            .semantic_search(query, search_limit, Some(session_id), options)
            .await
            .context("Failed to perform session semantic search")?;

        // Filter by similarity threshold (consistent with global search)
        let filtered_results: Vec<_> = results
            .into_iter()
            .filter(|r| r.similarity_score >= self.config.similarity_threshold)
            .collect();

        debug!(
            "Session semantic search returned {} results (filtered from threshold {})",
            filtered_results.len(),
            self.config.similarity_threshold
        );

        Ok(filtered_results)
    }

    /// Search within a specific set of sessions
    pub async fn semantic_search_multisession(
        &self,
        session_ids: &[Uuid],
        query: &str,
        limit: Option<usize>,
        date_range: Option<(chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)>,
        recency_bias: Option<f32>,
    ) -> Result<Vec<SemanticSearchResult>> {
        let search_limit = limit.unwrap_or(self.config.max_context_results);

        debug!(
            "Performing multisession semantic search in {} sessions for: '{}'",
            session_ids.len(),
            query
        );

        let options = crate::core::content_vectorizer::SearchOptions {
            limit: Some(search_limit),
            date_range,
            recency_bias,
        };

        let results = self
            .vectorizer
            .semantic_search_multisession(query, search_limit, session_ids, options)
            .await
            .context("Failed to perform multisession semantic search")?;

        // Filter by similarity threshold (consistent with global/session search)
        let filtered_results: Vec<_> = results
            .into_iter()
            .filter(|r| r.similarity_score >= self.config.similarity_threshold)
            .collect();

        debug!(
            "Multisession semantic search returned {} results (filtered from threshold {})",
            filtered_results.len(),
            self.config.similarity_threshold
        );

        Ok(filtered_results)
    }

    /// Find related experiences from other sessions
    pub async fn find_related_experiences(
        &self,
        current_session_id: Uuid,
        topic: &str,
    ) -> Result<Vec<RelatedExperience>> {
        if !self.config.cross_session_enabled {
            return Ok(Vec::new());
        }

        debug!(
            "Finding related experiences for topic: '{}' (excluding session {})",
            topic, current_session_id
        );

        // Get semantic search results
        let search_results = self
            .vectorizer
            .semantic_search(topic, self.config.max_related_experiences * 2, None, crate::core::content_vectorizer::SearchOptions::default())
            .await?;

        let mut related_experiences = Vec::new();

        for result in search_results {
            // Skip current session
            if result.session_id == current_session_id {
                continue;
            }

            // Skip if below similarity threshold
            if result.similarity_score < self.config.similarity_threshold {
                continue;
            }

            // Calculate relevance score (combination of similarity and importance)
            let relevance_score = result.combined_score;

            // Create context description
            let context = self
                .generate_context_description(&result)
                .await
                .unwrap_or_else(|_| "No context available".to_string());

            related_experiences.push(RelatedExperience {
                session_id: result.session_id,
                similarity_score: result.similarity_score,
                relevance_score,
                content_snippet: self.truncate_content(&result.text_content, 200),
                content_type: format!("{:?}", result.content_type),
                timestamp: result.timestamp,
                context,
            });

            if related_experiences.len() >= self.config.max_related_experiences {
                break;
            }
        }

        // Sort by relevance score
        related_experiences.sort_by(|a, b| {
            b.relevance_score
                .partial_cmp(&a.relevance_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        debug!("Found {} related experiences", related_experiences.len());
        Ok(related_experiences)
    }

    /// Enhanced context restoration with semantic understanding
    pub async fn enhanced_context_restore(
        &self,
        session_id: Uuid,
        context_hint: &str,
    ) -> Result<EnhancedContext> {
        info!(
            "Enhanced context restore for session {} with hint: '{}'",
            session_id, context_hint
        );

        // Search within current session
        let primary_content = self
            .semantic_search_session(session_id, context_hint, Some(10), None, None)
            .await?;

        // Find related experiences from other sessions
        let related_experiences = self
            .find_related_experiences(session_id, context_hint)
            .await?;

        // Extract semantic themes
        let semantic_themes = self.extract_semantic_themes(&primary_content, &related_experiences);

        // Generate suggested actions
        let suggested_actions =
            self.generate_suggested_actions(&primary_content, &related_experiences);

        // Calculate confidence score
        let confidence_score =
            self.calculate_context_confidence(&primary_content, &related_experiences);

        Ok(EnhancedContext {
            primary_content,
            related_experiences,
            confidence_score,
            semantic_themes,
            suggested_actions,
        })
    }

    /// Generate semantic insights for a given topic
    pub async fn generate_semantic_insights(
        &self,
        topic: &str,
        session_filter: Option<Uuid>,
    ) -> Result<SemanticInsights> {
        debug!("Generating semantic insights for topic: '{}'", topic);

        let search_results = if let Some(session_id) = session_filter {
            self.semantic_search_session(session_id, topic, Some(50), None, None)
                .await?
        } else {
            self.semantic_search_global(topic, Some(50), None, None).await?
        };

        // Group results by content type and analyze patterns
        let mut decision_points = Vec::new();
        let mut problems_solutions = Vec::new();
        let mut user_messages = Vec::new();

        for result in &search_results {
            match result.content_type {
                crate::core::content_vectorizer::ContentType::DecisionPoint => {
                    decision_points.push(&result.text_content);
                }
                crate::core::content_vectorizer::ContentType::ProblemSolution => {
                    problems_solutions.push(&result.text_content);
                }
                crate::core::content_vectorizer::ContentType::UserMessage => {
                    user_messages.push(&result.text_content);
                }
                _ => {}
            }
        }

        // Extract insights
        let dominant_themes = self.extract_themes_from_results(&search_results);
        let related_concepts = self.extract_related_concepts(&search_results);
        let user_patterns = self.analyze_user_patterns(&user_messages);
        let decision_history = self.analyze_decision_patterns(&decision_points);
        let problem_patterns = self.analyze_problem_patterns(&problems_solutions);
        let knowledge_gaps = self.identify_knowledge_gaps(&search_results);

        Ok(SemanticInsights {
            dominant_themes,
            related_concepts,
            user_patterns,
            decision_history,
            problem_patterns,
            knowledge_gaps,
        })
    }

    /// Get semantic similarity between two pieces of content
    /// Uses the embedding engine to compute cosine similarity between text embeddings
    pub async fn calculate_semantic_similarity(&self, text1: &str, text2: &str) -> Result<f32> {
        self.vectorizer
            .compute_text_similarity(text1, text2)
            .await
            .context("Failed to compute text similarity")
    }

    /// Find sessions with similar themes
    pub async fn find_similar_sessions(
        &self,
        reference_session_id: Uuid,
        limit: usize,
    ) -> Result<Vec<(Uuid, f32)>> {
        debug!("Finding sessions similar to {}", reference_session_id);

        // Use multiple semantic queries to get diverse content from the reference session
        // This replaces the meaningless "*" wildcard with actual semantic queries
        let semantic_queries = [
            "important decisions architecture implementation",
            "problems solutions issues fixes",
            "questions answers explanations",
            "code changes modifications updates",
        ];

        let mut all_reference_content = Vec::new();
        for query in &semantic_queries {
            let results = self
                .vectorizer
                .semantic_search(query, 5, Some(reference_session_id), crate::core::content_vectorizer::SearchOptions::default())
                .await
                .unwrap_or_default();
            all_reference_content.extend(results);
        }

        if all_reference_content.is_empty() {
            debug!("No content found in reference session {}", reference_session_id);
            return Ok(Vec::new());
        }

        // Deduplicate by content ID
        all_reference_content.sort_by(|a, b| a.content_id.cmp(&b.content_id));
        all_reference_content.dedup_by(|a, b| a.content_id == b.content_id);

        // Extract key topics from reference session
        let reference_themes = self.extract_themes_from_results(&all_reference_content);

        // Search for similar content in other sessions
        let mut session_similarities: HashMap<Uuid, Vec<f32>> = HashMap::new();

        for theme in &reference_themes {
            let similar_results = self.semantic_search_global(theme, Some(50), None, None).await?;

            for result in similar_results {
                if result.session_id != reference_session_id {
                    session_similarities
                        .entry(result.session_id)
                        .or_default()
                        .push(result.similarity_score);
                }
            }
        }

        // Calculate average similarity for each session
        let mut similar_sessions: Vec<(Uuid, f32)> = session_similarities
            .into_iter()
            .map(|(session_id, similarities)| {
                let avg_similarity = similarities.iter().sum::<f32>() / similarities.len() as f32;
                (session_id, avg_similarity)
            })
            .collect();

        // Sort by similarity and take top results
        similar_sessions.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        similar_sessions.truncate(limit);

        debug!("Found {} similar sessions", similar_sessions.len());
        Ok(similar_sessions)
    }

    // Helper methods

    async fn generate_context_description(&self, result: &SemanticSearchResult) -> Result<String> {
        // Generate a brief context description for the result
        Ok(format!(
            "From session {} ({:?}): {}",
            result.session_id,
            result.content_type,
            self.truncate_content(&result.text_content, 100)
        ))
    }

    fn truncate_content(&self, content: &str, max_chars: usize) -> String {
        let char_count = content.chars().count();
        if char_count <= max_chars {
            content.to_string()
        } else {
            let truncated: String = content.chars().take(max_chars).collect();
            format!("{}...", truncated)
        }
    }

    fn extract_semantic_themes(
        &self,
        primary: &[SemanticSearchResult],
        related: &[RelatedExperience],
    ) -> Vec<String> {
        let mut themes = Vec::new();

        // Extract themes from primary content
        for result in primary {
            if result.similarity_score > 0.8 {
                themes.push(format!("High relevance: {:?}", result.content_type));
            }
        }

        // Extract themes from related experiences
        for exp in related {
            if exp.similarity_score > 0.8 {
                themes.push(format!("Cross-session pattern: {}", exp.content_type));
            }
        }

        themes.truncate(5);
        themes
    }

    fn generate_suggested_actions(
        &self,
        primary: &[SemanticSearchResult],
        related: &[RelatedExperience],
    ) -> Vec<String> {
        let mut actions = Vec::new();

        if !primary.is_empty() {
            actions.push("Review recent session context".to_string());
        }

        if !related.is_empty() {
            actions.push("Consider insights from similar past experiences".to_string());
        }

        if related.len() > 3 {
            actions.push("Analyze recurring patterns across sessions".to_string());
        }

        actions
    }

    fn calculate_context_confidence(
        &self,
        primary: &[SemanticSearchResult],
        related: &[RelatedExperience],
    ) -> f32 {
        let primary_confidence = if primary.is_empty() {
            0.0
        } else {
            primary.iter().map(|r| r.similarity_score).sum::<f32>() / primary.len() as f32
        };

        let related_confidence = if related.is_empty() {
            0.0
        } else {
            related.iter().map(|r| r.similarity_score).sum::<f32>() / related.len() as f32
        };

        (primary_confidence * 0.7 + related_confidence * 0.3).min(1.0)
    }

    fn extract_themes_from_results(&self, results: &[SemanticSearchResult]) -> Vec<String> {
        let mut themes = Vec::new();

        for result in results {
            // Extract key words and phrases (simplified)
            let words: Vec<&str> = result
                .text_content
                .split_whitespace()
                .filter(|w| w.len() > 3)
                .take(3)
                .collect();

            if !words.is_empty() {
                themes.push(words.join(" "));
            }
        }

        themes.truncate(10);
        themes
    }

    fn extract_related_concepts(&self, results: &[SemanticSearchResult]) -> Vec<String> {
        // Extract concepts based on content types and similarity patterns
        let mut concepts = Vec::new();

        let decision_count = results
            .iter()
            .filter(|r| {
                matches!(
                    r.content_type,
                    crate::core::content_vectorizer::ContentType::DecisionPoint
                )
            })
            .count();

        if decision_count > 0 {
            concepts.push("Decision making patterns".to_string());
        }

        concepts.truncate(5);
        concepts
    }

    fn analyze_user_patterns(&self, user_messages: &[&String]) -> Vec<String> {
        let mut patterns = Vec::new();

        if user_messages.len() > 3 {
            patterns.push("Frequent questioning pattern".to_string());
        }

        patterns
    }

    fn analyze_decision_patterns(&self, decisions: &[&String]) -> Vec<String> {
        let mut patterns = Vec::new();

        if decisions.len() > 2 {
            patterns.push("Multiple decision points identified".to_string());
        }

        patterns
    }

    fn analyze_problem_patterns(&self, problems: &[&String]) -> Vec<String> {
        let mut patterns = Vec::new();

        if problems.len() > 1 {
            patterns.push("Recurring problem-solving approach".to_string());
        }

        patterns
    }

    fn identify_knowledge_gaps(&self, results: &[SemanticSearchResult]) -> Vec<String> {
        let mut gaps = Vec::new();

        if results.is_empty() {
            gaps.push("Limited context available for this topic".to_string());
        }

        gaps
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_semantic_query_config_defaults() {
        let config = SemanticQueryConfig::default();
        assert_eq!(config.max_related_experiences, 10);
        assert_eq!(config.similarity_threshold, 0.7);
        assert!(config.cross_session_enabled);
    }

    #[test]
    fn test_truncate_content() {
        // Test the character-based truncation logic
        // Helper function mirroring the actual implementation
        fn truncate(content: &str, max_chars: usize) -> String {
            let char_count = content.chars().count();
            if char_count <= max_chars {
                content.to_string()
            } else {
                let truncated: String = content.chars().take(max_chars).collect();
                format!("{}...", truncated)
            }
        }

        // Test ASCII content
        let content = "This is a very long piece of content that should be truncated";
        assert_eq!(truncate(content, 20), "This is a very long ...");

        // Test content shorter than max
        assert_eq!(truncate("short", 20), "short");

        // Test Unicode content (Cyrillic) - this would panic with byte indexing
        let unicode_content = "Привет мир! Это тестовое сообщение.";
        let truncated_unicode = truncate(unicode_content, 10);
        assert_eq!(truncated_unicode, "Привет мир...");
        assert_eq!(truncated_unicode.chars().count(), 13); // 10 chars + "..."

        // Test emoji content - multi-byte UTF-8
        let emoji_content = "Hello 🌍🌎🌏 World!";
        let truncated_emoji = truncate(emoji_content, 10);
        assert_eq!(truncated_emoji, "Hello 🌍🌎🌏 ...");
    }

    #[test]
    fn test_related_experience_creation() {
        let experience = RelatedExperience {
            session_id: Uuid::new_v4(),
            similarity_score: 0.85,
            relevance_score: 0.90,
            content_snippet: "Test content".to_string(),
            content_type: "DecisionPoint".to_string(),
            timestamp: Utc::now(),
            context: "Test context".to_string(),
        };

        assert!(experience.similarity_score > 0.8);
        assert!(experience.relevance_score > 0.8);
    }
}
