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
pub mod presentation;

pub use presentation::{
    ConceptSummary, DecisionSummary, EntitySummary, QuestionSummary, SessionStats,
    StructuredSummaryView,
};

use crate::session::active_session::ActiveSession;
use chrono::Utc;

/// Options for filtering and limiting summary output
#[derive(Debug, Clone)]
#[derive(Default)]
pub struct SummaryOptions {
    pub decisions_limit: Option<usize>,
    pub entities_limit: Option<usize>,
    pub questions_limit: Option<usize>,
    pub concepts_limit: Option<usize>,
    pub min_confidence: Option<f32>,
    pub compact: bool,
}


impl SummaryOptions {
    /// Create compact mode options (returns minimal data)
    pub fn compact() -> Self {
        Self {
            decisions_limit: Some(10), // Increased from 5
            entities_limit: Some(15),  // Increased from 10
            questions_limit: Some(5),
            concepts_limit: Some(5),
            min_confidence: Some(0.4), // Lowered from 0.6 to include more decisions
            compact: true,
        }
    }

    /// Create default options with limits
    pub fn with_limits(
        decisions: usize,
        entities: usize,
        questions: usize,
        concepts: usize,
    ) -> Self {
        Self {
            decisions_limit: Some(decisions),
            entities_limit: Some(entities),
            questions_limit: Some(questions),
            concepts_limit: Some(concepts),
            min_confidence: None,
            compact: false,
        }
    }
}

/// Main summary generator that uses existing structured data
/// All methods are associated functions (no state needed)
pub struct SummaryGenerator;

impl SummaryGenerator {
    /// Generate structured summary from existing ActiveSession data
    pub fn generate_structured_summary(session: &ActiveSession) -> StructuredSummaryView {
        Self::generate_structured_summary_filtered(session, &SummaryOptions::default())
    }

    /// Generate filtered/limited structured summary
    pub fn generate_structured_summary_filtered(
        session: &ActiveSession,
        options: &SummaryOptions,
    ) -> StructuredSummaryView {
        let session_stats = Self::calculate_session_stats(session);

        // Get entity limit for both summaries and names
        let entity_limit = if options.compact {
            10
        } else {
            options.entities_limit.unwrap_or(20)
        };

        // Single pass: get entity analysis, truncate, then derive names
        let mut entity_analysis = session.entity_graph.analyze_entity_importance();
        entity_analysis.truncate(entity_limit);

        // Derive important_entities from entity_analysis (already sorted by importance)
        let important_entities: Vec<String> = entity_analysis
            .iter()
            .map(|e| e.entity_name.clone())
            .collect();

        // Filter and limit decisions
        let mut key_decisions: Vec<DecisionSummary> = session
            .current_state
            .key_decisions
            .iter()
            .filter(|d| {
                if let Some(min_conf) = options.min_confidence {
                    d.confidence >= min_conf
                } else {
                    true
                }
            })
            .map(DecisionSummary::from_decision_item)
            .collect();

        // Sort by confidence (highest first) then by timestamp
        key_decisions.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.timestamp.cmp(&a.timestamp))
        });

        if let Some(limit) = options.decisions_limit {
            key_decisions.truncate(limit);
        }

        // Limit questions
        let mut open_questions: Vec<QuestionSummary> = session
            .current_state
            .open_questions
            .iter()
            .map(QuestionSummary::from_question_item)
            .collect();

        // Sort by urgency and recency
        open_questions.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

        if let Some(limit) = options.questions_limit {
            open_questions.truncate(limit);
        }

        // Limit concepts
        let mut key_concepts: Vec<ConceptSummary> = session
            .current_state
            .key_concepts
            .iter()
            .map(ConceptSummary::from_concept_item)
            .collect();

        // Sort by timestamp (most recent first)
        key_concepts.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

        if let Some(limit) = options.concepts_limit {
            key_concepts.truncate(limit);
        }

        StructuredSummaryView {
            session_id: session.id(),
            generated_at: Utc::now(),

            // Filtered and limited data
            key_decisions,
            open_questions,
            key_concepts,

            // From SimpleEntityGraph (limited)
            important_entities,
            entity_summaries: entity_analysis
                .into_iter()
                .map(|analysis| EntitySummary::from_entity_analysis(&analysis))
                .collect(),

            // Session metadata
            session_stats,
        }
    }

    /// Calculate session statistics from existing data
    /// This is a lightweight operation that doesn't require full summary generation
    pub fn calculate_session_stats(session: &ActiveSession) -> SessionStats {
        use crate::summary::presentation::SessionStatsBuilder;

        SessionStatsBuilder::new(session.id(), session.created_at(), session.last_updated)
            .with_context_sizes(
                session.hot_context.len(),
                session.warm_context.len(),
                session.cold_context.len(),
            )
            .with_counts(
                session.incremental_updates.len(),
                session.entity_graph.entities.len(),
                session.current_state.key_decisions.len(),
            )
            .with_references(
                session.current_state.open_questions.len(),
                session.current_state.key_concepts.len(),
                session.code_references.values().map(|v| v.len()).sum(),
            )
            .build()
    }

    /// Estimate summary size in tokens without full generation
    /// Returns (estimated_tokens, should_use_compact)
    pub fn estimate_summary_size(session: &ActiveSession, max_tokens: usize) -> (usize, bool) {
        // Average sizes per item (empirically determined from JSON serialization)
        const DECISION_AVG_TOKENS: usize = 150;
        const ENTITY_AVG_TOKENS: usize = 80;
        const QUESTION_AVG_TOKENS: usize = 100;
        const CONCEPT_AVG_TOKENS: usize = 120;
        const BASE_OVERHEAD_TOKENS: usize = 500;

        let decision_count = session.current_state.key_decisions.len();
        let entity_count = session.entity_graph.entities.len();
        let question_count = session.current_state.open_questions.len();
        let concept_count = session.current_state.key_concepts.len();

        let estimated_tokens = BASE_OVERHEAD_TOKENS
            + (decision_count * DECISION_AVG_TOKENS)
            + (entity_count * ENTITY_AVG_TOKENS)
            + (question_count * QUESTION_AVG_TOKENS)
            + (concept_count * CONCEPT_AVG_TOKENS);

        (estimated_tokens, estimated_tokens > max_tokens)
    }

    /// Extract key insights from existing data
    pub fn extract_key_insights(session: &ActiveSession, limit: usize) -> Vec<String> {
        let mut insights = Vec::new();

        // Insights from decisions
        for decision in &session.current_state.key_decisions {
            if decision.confidence > 0.8 {
                insights.push(format!(
                    "High-confidence decision: {}",
                    decision.description
                ));
            }
        }

        // Insights from entity importance
        let top_entities = session.entity_graph.get_most_important_entities(3);
        if !top_entities.is_empty() {
            let entity_names: Vec<String> = top_entities.iter().map(|e| e.name.clone()).collect();
            let entity_list = entity_names.join(", ");
            insights.push(format!("Primary focus areas: {}", entity_list));
        }

        // Insights from update patterns
        let total_updates = session.incremental_updates.len();
        if total_updates > 10 {
            insights.push(format!(
                "Comprehensive discussion with {} updates",
                total_updates
            ));
        }

        // Insights from code references
        let code_files: Vec<_> = session.code_references.keys().collect();
        if code_files.len() > 1 {
            insights.push(format!(
                "Multi-file code analysis covering {} files",
                code_files.len()
            ));
        }

        // Limit results
        insights.truncate(limit);
        insights
    }

    /// Extract decision timeline from existing data
    pub fn extract_decision_timeline(session: &ActiveSession) -> Vec<DecisionSummary> {
        let mut decisions: Vec<_> = session
            .current_state
            .key_decisions
            .iter()
            .map(DecisionSummary::from_decision_item)
            .collect();

        // Sort by timestamp
        decisions.sort_by_key(|d| d.timestamp);
        decisions
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::active_session::ActiveSession;
    use uuid::Uuid;

    #[test]
    fn test_summary_generation() {
        let session = ActiveSession::new(
            Uuid::new_v4(),
            Some("Test Session".to_string()),
            Some("Test session for summary generation".to_string()),
        );

        let summary = SummaryGenerator::generate_structured_summary(&session);

        assert_eq!(summary.session_id, session.id());
        assert!(summary.generated_at <= Utc::now());
        assert_eq!(summary.session_stats.hot_context_size, 0); // Empty session
    }

    #[test]
    fn test_key_insights_extraction() {
        let session = ActiveSession::new(
            Uuid::new_v4(),
            Some("Test Session".to_string()),
            Some("Test session for insights".to_string()),
        );

        let insights = SummaryGenerator::extract_key_insights(&session, 5);

        // Empty session should have minimal insights
        assert!(insights.len() <= 5);
    }
}
