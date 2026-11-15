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
//! Summary generation and presentation types.
//!
//! Provides the `SummaryGenerator` which produces structured summary views
//! from an `ActiveSession`, along with filtering options.
pub mod presentation;

pub use presentation::{
    ConceptSummary, DecisionSummary, EntitySummary, ImportanceLevel, QuestionSummary, SessionStats,
    StructuredSummaryView,
};

use crate::core::structured_context::{ConceptItem, DecisionItem, QuestionItem};
use crate::graph::entity_graph::top_k_by;
use crate::session::active_session::ActiveSession;
use chrono::{DateTime, Utc};

/// Sorted top `limit` of `items` (all of them, sorted, when `limit` is None).
fn top_k<T>(
    items: Vec<T>,
    limit: Option<usize>,
    cmp: impl FnMut(&T, &T) -> std::cmp::Ordering,
) -> Vec<T> {
    top_k_by(items, limit.unwrap_or(usize::MAX), cmp)
}

/// Options for filtering and limiting summary output
#[derive(Debug, Clone, Default)]
pub struct SummaryOptions {
    /// Maximum number of decisions to include
    pub decisions_limit: Option<usize>,
    /// Maximum number of entities to include
    pub entities_limit: Option<usize>,
    /// Maximum number of questions to include
    pub questions_limit: Option<usize>,
    /// Maximum number of concepts to include
    pub concepts_limit: Option<usize>,
    /// Minimum confidence threshold for decisions
    pub min_confidence: Option<f32>,
    /// Whether to produce a compact (minimal) summary
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
    /// Generate structured summary from existing ActiveSession data, as of
    /// `now` (ages and recency are measured from it).
    pub fn generate_structured_summary(
        session: &ActiveSession,
        now: DateTime<Utc>,
    ) -> StructuredSummaryView {
        Self::generate_structured_summary_filtered(session, &SummaryOptions::default(), now)
    }

    /// Generate filtered/limited structured summary, as of `now`.
    pub fn generate_structured_summary_filtered(
        session: &ActiveSession,
        options: &SummaryOptions,
        now: DateTime<Utc>,
    ) -> StructuredSummaryView {
        let session_stats = Self::calculate_session_stats(session);

        // Get entity limit for both summaries and names
        let entity_limit = if options.compact {
            10
        } else {
            options.entities_limit.unwrap_or(20)
        };

        // Top-k entity analysis: only the survivors are materialised.
        let entity_analysis = session
            .entity_graph()
            .analyze_entity_importance_top(entity_limit);

        // Derive important_entities from entity_analysis (already sorted by importance)
        let important_entities: Vec<String> = entity_analysis
            .iter()
            .map(|e| e.entity_name.clone())
            .collect();

        let state = session.current_state();

        // Filter, then select the top decisions by reference; clone only
        // the survivors. Order: confidence (highest first), then timestamp
        // (newest first), then original position (keeps the previous
        // stable-sort order for full ties).
        let decisions: Vec<(usize, &DecisionItem)> = state
            .key_decisions
            .iter()
            .enumerate()
            .filter(|(_, d)| options.min_confidence.is_none_or(|min| d.confidence >= min))
            .collect();
        let key_decisions: Vec<DecisionSummary> =
            top_k(decisions, options.decisions_limit, |(ia, a), (ib, b)| {
                b.confidence
                    .total_cmp(&a.confidence)
                    .then_with(|| b.timestamp.cmp(&a.timestamp))
                    .then_with(|| ia.cmp(ib))
            })
            .into_iter()
            .map(|(_, d)| DecisionSummary::from_decision_item(d))
            .collect();

        // Questions and concepts: most recent first.
        let answered_questions: Vec<QuestionSummary> = top_k(
            state.answered_questions.iter().enumerate().collect(),
            options.questions_limit,
            |(ia, a): &(usize, &QuestionItem), (ib, b)| {
                b.timestamp.cmp(&a.timestamp).then_with(|| ia.cmp(ib))
            },
        )
        .into_iter()
        .map(|(_, q)| QuestionSummary::from_question_item(q, now))
        .collect();

        let key_concepts: Vec<ConceptSummary> = top_k(
            state.key_concepts.iter().enumerate().collect(),
            options.concepts_limit,
            |(ia, a): &(usize, &ConceptItem), (ib, b)| {
                b.timestamp.cmp(&a.timestamp).then_with(|| ia.cmp(ib))
            },
        )
        .into_iter()
        .map(|(_, c)| ConceptSummary::from_concept_item(c))
        .collect();

        StructuredSummaryView {
            session_id: session.id(),
            generated_at: now,

            // Filtered and limited data
            key_decisions,
            answered_questions,
            key_concepts,

            // From SimpleEntityGraph (limited)
            important_entities,
            entity_summaries: entity_analysis
                .into_iter()
                .map(|analysis| EntitySummary::from_entity_analysis(&analysis, now))
                .collect(),

            // Session metadata
            session_stats,
        }
    }

    /// Calculate session statistics from existing data
    /// This is a lightweight operation that doesn't require full summary generation
    pub fn calculate_session_stats(session: &ActiveSession) -> SessionStats {
        use crate::summary::presentation::SessionStatsBuilder;

        let state = session.current_state();
        SessionStatsBuilder::new(session.id(), session.created_at(), session.last_updated())
            // The warm and cold tiers are gone: the hot window is the
            // newest records, and every record is in the list.
            .with_context_sizes(session.hot_records().len(), 0, 0)
            .with_counts(
                session.records().len(),
                session.entity_graph().entities.len(),
                state.key_decisions.len(),
            )
            .with_references(
                state.answered_questions.len(),
                state.key_concepts.len(),
                session
                    .records()
                    .iter()
                    .filter(|u| u.related_code.is_some())
                    .count(),
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

        let state = session.current_state();
        let decision_count = state.key_decisions.len();
        let entity_count = session.entity_graph().entities.len();
        let question_count = state.answered_questions.len();
        let concept_count = state.key_concepts.len();

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
        for decision in &session.current_state().key_decisions {
            if decision.confidence > 0.8 {
                insights.push(format!(
                    "High-confidence decision: {}",
                    decision.description
                ));
            }
        }

        // Insights from entity importance
        let top_entities = session.entity_graph().get_most_important_entities(3);
        if !top_entities.is_empty() {
            let entity_names: Vec<String> = top_entities.iter().map(|e| e.name.clone()).collect();
            let entity_list = entity_names.join(", ");
            insights.push(format!("Primary focus areas: {}", entity_list));
        }

        // Insights from update patterns
        let total_updates = session.records().len();
        if total_updates > 10 {
            insights.push(format!(
                "Comprehensive discussion with {} updates",
                total_updates
            ));
        }

        // Insights from code references
        let code_files = session.code_references().len();
        if code_files > 1 {
            insights.push(format!(
                "Multi-file code analysis covering {} files",
                code_files
            ));
        }

        // Limit results
        insights.truncate(limit);
        insights
    }

    /// Extract decision timeline from existing data
    pub fn extract_decision_timeline(session: &ActiveSession) -> Vec<DecisionSummary> {
        let mut decisions: Vec<_> = session
            .current_state()
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
mod tests;
