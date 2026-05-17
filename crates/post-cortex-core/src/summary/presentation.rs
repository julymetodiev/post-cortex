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
use crate::core::structured_context::{ConceptItem, DecisionItem, QuestionItem, QuestionStatus};
use crate::graph::entity_graph::EntityAnalysis;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Main structured summary view that combines all existing data
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct StructuredSummaryView {
    pub session_id: Uuid,
    pub generated_at: DateTime<Utc>,

    // From StructuredContext
    pub key_decisions: Vec<DecisionSummary>,
    pub open_questions: Vec<QuestionSummary>,
    pub key_concepts: Vec<ConceptSummary>,

    // From SimpleEntityGraph
    pub important_entities: Vec<String>,
    pub entity_summaries: Vec<EntitySummary>,

    // Session metadata
    pub session_stats: SessionStats,
}

/// Decision summary from DecisionItem
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DecisionSummary {
    pub description: String,
    pub context: String,
    pub alternatives: Vec<String>,
    pub confidence: f32,
    pub timestamp: DateTime<Utc>,
    pub confidence_level: ConfidenceLevel,
}

impl DecisionSummary {
    pub fn from_decision_item(decision: &DecisionItem) -> Self {
        let confidence_level = match decision.confidence {
            f if f >= 0.8 => ConfidenceLevel::High,
            f if f >= 0.6 => ConfidenceLevel::Medium,
            f if f >= 0.4 => ConfidenceLevel::Low,
            _ => ConfidenceLevel::VeryLow,
        };

        Self {
            description: decision.description.clone(),
            context: decision.context.clone(),
            alternatives: decision.alternatives.clone(),
            confidence: decision.confidence,
            timestamp: decision.timestamp,
            confidence_level,
        }
    }
}

/// Question summary from QuestionItem
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct QuestionSummary {
    pub question: String,
    pub context: String,
    pub status: QuestionStatus,
    pub timestamp: DateTime<Utc>,
    pub last_updated: DateTime<Utc>,
    pub days_open: i64,
    pub urgency_level: UrgencyLevel,
}

impl QuestionSummary {
    pub fn from_question_item(question: &QuestionItem) -> Self {
        let now = Utc::now();
        let days_open = (now - question.timestamp).num_days();

        let urgency_level = match (&question.status, days_open) {
            (QuestionStatus::Open, days) if days > 7 => UrgencyLevel::High,
            (QuestionStatus::Open, days) if days > 3 => UrgencyLevel::Medium,
            (QuestionStatus::Open, _) => UrgencyLevel::Low,
            (QuestionStatus::InProgress, days) if days > 14 => UrgencyLevel::High,
            (QuestionStatus::InProgress, _) => UrgencyLevel::Medium,
            _ => UrgencyLevel::Low,
        };

        Self {
            question: question.question.clone(),
            context: question.context.clone(),
            status: question.status.clone(),
            timestamp: question.timestamp,
            last_updated: question.last_updated,
            days_open,
            urgency_level,
        }
    }
}

/// Concept summary from ConceptItem
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ConceptSummary {
    pub name: String,
    pub definition: String,
    pub examples: Vec<String>,
    pub related_concepts: Vec<String>,
    pub timestamp: DateTime<Utc>,
    pub complexity_level: ComplexityLevel,
}

impl ConceptSummary {
    pub fn from_concept_item(concept: &ConceptItem) -> Self {
        let complexity_level = match (concept.examples.len(), concept.related_concepts.len()) {
            (examples, related) if examples > 3 && related > 5 => ComplexityLevel::High,
            (examples, related) if examples > 1 && related > 2 => ComplexityLevel::Medium,
            _ => ComplexityLevel::Low,
        };

        Self {
            name: concept.name.clone(),
            definition: concept.definition.clone(),
            examples: concept.examples.clone(),
            related_concepts: concept.related_concepts.clone(),
            timestamp: concept.timestamp,
            complexity_level,
        }
    }
}

/// Entity summary from EntityAnalysis
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct EntitySummary {
    pub entity_name: String,
    pub importance_score: f32,
    pub mention_count: u32,
    pub relationship_count: usize,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
    pub importance_level: ImportanceLevel,
    pub recency_level: RecencyLevel,
}

impl EntitySummary {
    pub fn from_entity_analysis(analysis: &EntityAnalysis) -> Self {
        let importance_level = match analysis.importance_score {
            score if score >= 2.0 => ImportanceLevel::Critical,
            score if score >= 1.5 => ImportanceLevel::High,
            score if score >= 1.0 => ImportanceLevel::Medium,
            score if score >= 0.5 => ImportanceLevel::Low,
            _ => ImportanceLevel::Minimal,
        };

        let now = Utc::now();
        let days_since_last_seen = (now - analysis.last_seen).num_days();
        let recency_level = match days_since_last_seen {
            0 => RecencyLevel::Today,
            1..=3 => RecencyLevel::Recent,
            4..=7 => RecencyLevel::ThisWeek,
            8..=30 => RecencyLevel::ThisMonth,
            _ => RecencyLevel::Old,
        };

        Self {
            entity_name: analysis.entity_name.clone(),
            importance_score: analysis.importance_score,
            mention_count: analysis.mention_count,
            relationship_count: analysis.relationship_count,
            first_seen: analysis.first_seen,
            last_seen: analysis.last_seen,
            importance_level,
            recency_level,
        }
    }
}

/// Session statistics from ActiveSession data
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SessionStats {
    pub session_id: Uuid,
    pub hot_context_size: usize,
    pub warm_context_size: usize,
    pub cold_context_size: usize,
    pub total_updates: usize,
    pub entity_count: usize,
    pub decision_count: usize,
    pub open_question_count: usize,
    pub concept_count: usize,
    pub code_reference_count: usize,
    pub created_at: DateTime<Utc>,
    pub last_updated: DateTime<Utc>,
    pub session_duration: SessionDuration,
    pub activity_level: ActivityLevel,
}

/// Builder pattern for SessionStats
pub struct SessionStatsBuilder {
    session_id: Uuid,
    hot_context_size: usize,
    warm_context_size: usize,
    cold_context_size: usize,
    total_updates: usize,
    entity_count: usize,
    decision_count: usize,
    open_question_count: usize,
    concept_count: usize,
    code_reference_count: usize,
    created_at: DateTime<Utc>,
    last_updated: DateTime<Utc>,
}

impl SessionStatsBuilder {
    pub fn new(session_id: Uuid, created_at: DateTime<Utc>, last_updated: DateTime<Utc>) -> Self {
        Self {
            session_id,
            hot_context_size: 0,
            warm_context_size: 0,
            cold_context_size: 0,
            total_updates: 0,
            entity_count: 0,
            decision_count: 0,
            open_question_count: 0,
            concept_count: 0,
            code_reference_count: 0,
            created_at,
            last_updated,
        }
    }

    pub fn with_context_sizes(mut self, hot: usize, warm: usize, cold: usize) -> Self {
        self.hot_context_size = hot;
        self.warm_context_size = warm;
        self.cold_context_size = cold;
        self
    }

    pub fn with_counts(mut self, updates: usize, entities: usize, decisions: usize) -> Self {
        self.total_updates = updates;
        self.entity_count = entities;
        self.decision_count = decisions;
        self
    }

    pub fn with_references(mut self, questions: usize, concepts: usize, code_refs: usize) -> Self {
        self.open_question_count = questions;
        self.concept_count = concepts;
        self.code_reference_count = code_refs;
        self
    }

    pub fn build(self) -> SessionStats {
        SessionStats::from_builder(self)
    }
}

impl SessionStats {
    fn from_builder(builder: SessionStatsBuilder) -> Self {
        let duration_hours = (builder.last_updated - builder.created_at).num_hours();
        let session_duration = match duration_hours {
            0..=1 => SessionDuration::Short,
            2..=4 => SessionDuration::Medium,
            5..=8 => SessionDuration::Long,
            _ => SessionDuration::Extended,
        };

        let activity_level = match (builder.total_updates, duration_hours.max(1)) {
            (updates, hours) if updates as i64 / hours > 10 => ActivityLevel::VeryHigh,
            (updates, hours) if updates as i64 / hours > 5 => ActivityLevel::High,
            (updates, hours) if updates as i64 / hours > 2 => ActivityLevel::Medium,
            (updates, hours) if updates as i64 / hours > 0 => ActivityLevel::Low,
            _ => ActivityLevel::Minimal,
        };

        Self {
            session_id: builder.session_id,
            hot_context_size: builder.hot_context_size,
            warm_context_size: builder.warm_context_size,
            cold_context_size: builder.cold_context_size,
            total_updates: builder.total_updates,
            entity_count: builder.entity_count,
            decision_count: builder.decision_count,
            open_question_count: builder.open_question_count,
            concept_count: builder.concept_count,
            code_reference_count: builder.code_reference_count,
            created_at: builder.created_at,
            last_updated: builder.last_updated,
            session_duration,
            activity_level,
        }
    }
}

/// Confidence level for decisions
#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum ConfidenceLevel {
    VeryLow, // 0.0 - 0.4
    Low,     // 0.4 - 0.6
    Medium,  // 0.6 - 0.8
    High,    // 0.8 - 1.0
}

/// Urgency level for questions
#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum UrgencyLevel {
    Low,
    Medium,
    High,
}

/// Complexity level for concepts
#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum ComplexityLevel {
    Low,
    Medium,
    High,
}

/// Importance level for entities
#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum ImportanceLevel {
    Minimal,  // < 0.5
    Low,      // 0.5 - 1.0
    Medium,   // 1.0 - 1.5
    High,     // 1.5 - 2.0
    Critical, // >= 2.0
}

/// Recency level for entities
#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum RecencyLevel {
    Today,
    Recent,    // 1-3 days
    ThisWeek,  // 4-7 days
    ThisMonth, // 8-30 days
    Old,       // > 30 days
}

/// Session duration categorization
#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum SessionDuration {
    Short,    // 0-1 hours
    Medium,   // 2-4 hours
    Long,     // 5-8 hours
    Extended, // > 8 hours
}

/// Activity level categorization
#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum ActivityLevel {
    Minimal,  // < 1 update per hour
    Low,      // 1-2 updates per hour
    Medium,   // 2-5 updates per hour
    High,     // 5-10 updates per hour
    VeryHigh, // > 10 updates per hour
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_confidence_level_mapping() {
        let decision = DecisionItem {
            description: "Test decision".to_string(),
            context: "Test context".to_string(),
            alternatives: vec![],
            confidence: 0.9,
            timestamp: Utc::now(),
        };

        let summary = DecisionSummary::from_decision_item(&decision);
        assert!(matches!(summary.confidence_level, ConfidenceLevel::High));
    }

    #[test]
    fn test_urgency_level_calculation() {
        let old_timestamp = Utc::now() - chrono::Duration::days(10);
        let question = QuestionItem {
            question: "Test question".to_string(),
            context: "Test context".to_string(),
            status: QuestionStatus::Open,
            timestamp: old_timestamp,
            last_updated: old_timestamp,
        };

        let summary = QuestionSummary::from_question_item(&question);
        assert!(summary.days_open >= 10);
        assert!(matches!(summary.urgency_level, UrgencyLevel::High));
    }

    #[test]
    fn test_session_duration_calculation() {
        let created = Utc::now() - chrono::Duration::hours(3);
        let updated = Utc::now();

        let stats = SessionStatsBuilder::new(Uuid::new_v4(), created, updated)
            .with_context_sizes(10, 5, 2)
            .with_counts(15, 20, 3)
            .with_references(2, 5, 1)
            .build();

        assert!(matches!(stats.session_duration, SessionDuration::Medium));
    }
}
