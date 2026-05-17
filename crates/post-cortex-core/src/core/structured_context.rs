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

//! Structured context types for organizing conversation knowledge.
//!
//! This module provides data structures for tracking decisions, questions,
//! concepts, specifications, and conversation flow within a session.

use chrono::{DateTime, Utc};

/// Maximum number of flow items to retain in conversation_flow.
/// Older items are removed when this limit is exceeded.
pub const MAX_FLOW_ITEMS: usize = 500;

/// Structured representation of conversation context.
///
/// Organizes knowledge extracted from conversations into queryable categories
/// including decisions, questions, concepts, and technical specifications.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StructuredContext {
    /// Key decisions made during the conversation with their rationale.
    pub key_decisions: Vec<DecisionItem>,

    /// Questions that have been asked, with their current status.
    pub open_questions: Vec<QuestionItem>,

    /// Important concepts identified and defined during discussion.
    pub key_concepts: Vec<ConceptItem>,

    /// Technical specifications and requirements discussed.
    pub technical_specifications: Vec<SpecItem>,

    /// Flow of the conversation tracking major steps and outcomes.
    pub conversation_flow: Vec<FlowItem>,

    // Note: action_items and references fields were removed as dead code.
    // They were never populated anywhere in the codebase.
    // Serde will ignore these fields when deserializing old data.
}

/// A decision made during the conversation.
///
/// Tracks what was decided, the context around the decision,
/// alternatives considered, and confidence level.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DecisionItem {
    /// Description of the decision made.
    pub description: String,

    /// Context explaining why this decision was needed.
    pub context: String,

    /// Alternative options that were considered.
    pub alternatives: Vec<String>,

    /// Confidence level in the decision (0.0 to 1.0).
    /// Values outside this range are clamped automatically.
    pub confidence: f32,

    /// When the decision was made.
    pub timestamp: DateTime<Utc>,
}

impl DecisionItem {
    /// Creates a new DecisionItem with validated confidence.
    ///
    /// The confidence value is clamped to the range [0.0, 1.0].
    pub fn new(
        description: String,
        context: String,
        alternatives: Vec<String>,
        confidence: f32,
        timestamp: DateTime<Utc>,
    ) -> Self {
        Self {
            description,
            context,
            alternatives,
            confidence: confidence.clamp(0.0, 1.0),
            timestamp,
        }
    }
}

impl Default for DecisionItem {
    fn default() -> Self {
        Self {
            description: String::new(),
            context: String::new(),
            alternatives: Vec::new(),
            confidence: 0.5,
            timestamp: Utc::now(),
        }
    }
}

/// A question raised during the conversation.
///
/// Tracks the question text, its context, and current resolution status.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct QuestionItem {
    /// The question that was asked.
    pub question: String,

    /// Additional context around the question.
    pub context: String,

    /// Current status of the question.
    pub status: QuestionStatus,

    /// When the question was first asked.
    pub timestamp: DateTime<Utc>,

    /// When the question status was last updated.
    pub last_updated: DateTime<Utc>,
}

impl Default for QuestionItem {
    fn default() -> Self {
        let now = Utc::now();
        Self {
            question: String::new(),
            context: String::new(),
            status: QuestionStatus::Open,
            timestamp: now,
            last_updated: now,
        }
    }
}

/// Status of a question in the conversation.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum QuestionStatus {
    /// Question is open and awaiting answer.
    Open,
    /// Question is being actively worked on.
    InProgress,
    /// Question has been answered.
    Answered,
    /// Question has been deferred for later.
    Deferred,
}

impl Default for QuestionStatus {
    fn default() -> Self {
        Self::Open
    }
}

/// A concept identified during the conversation.
///
/// Captures definitions, examples, and relationships to other concepts.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ConceptItem {
    /// Name of the concept.
    pub name: String,

    /// Definition or explanation of the concept.
    pub definition: String,

    /// Examples illustrating the concept.
    pub examples: Vec<String>,

    /// Names of related concepts.
    pub related_concepts: Vec<String>,

    /// When the concept was identified.
    pub timestamp: DateTime<Utc>,
}

impl Default for ConceptItem {
    fn default() -> Self {
        Self {
            name: String::new(),
            definition: String::new(),
            examples: Vec::new(),
            related_concepts: Vec::new(),
            timestamp: Utc::now(),
        }
    }
}

/// A technical specification discussed in the conversation.
///
/// Captures requirements and constraints for a technical component.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SpecItem {
    /// Title of the specification.
    pub title: String,

    /// Detailed description of the specification.
    pub description: String,

    /// List of requirements.
    pub requirements: Vec<String>,

    /// List of constraints or limitations.
    pub constraints: Vec<String>,

    /// When the specification was created.
    pub timestamp: DateTime<Utc>,
}

impl Default for SpecItem {
    fn default() -> Self {
        Self {
            title: String::new(),
            description: String::new(),
            requirements: Vec::new(),
            constraints: Vec::new(),
            timestamp: Utc::now(),
        }
    }
}

/// A step in the conversation flow.
///
/// Tracks the progression of the conversation including
/// what happened at each step and related context updates.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FlowItem {
    /// Description of this step in the conversation.
    pub step_description: String,

    /// When this step occurred.
    pub timestamp: DateTime<Utc>,

    /// IDs of context updates related to this step.
    pub related_updates: Vec<uuid::Uuid>,

    /// Outcome or result of this step, if any.
    pub outcome: Option<String>,
}

impl Default for FlowItem {
    fn default() -> Self {
        Self {
            step_description: String::new(),
            timestamp: Utc::now(),
            related_updates: Vec::new(),
            outcome: None,
        }
    }
}

impl Default for StructuredContext {
    fn default() -> Self {
        Self::new()
    }
}

impl StructuredContext {
    /// Creates a new empty StructuredContext.
    pub fn new() -> Self {
        Self {
            key_decisions: Vec::new(),
            open_questions: Vec::new(),
            key_concepts: Vec::new(),
            technical_specifications: Vec::new(),
            conversation_flow: Vec::new(),
        }
    }

    /// Adds a flow item, enforcing the maximum limit.
    ///
    /// When the limit is reached, the oldest items are removed
    /// to make room for new ones.
    pub fn add_flow_item(&mut self, item: FlowItem) {
        if self.conversation_flow.len() >= MAX_FLOW_ITEMS {
            // Remove oldest 10% to avoid frequent removals
            let remove_count = MAX_FLOW_ITEMS / 10;
            self.conversation_flow.drain(0..remove_count);
        }
        self.conversation_flow.push(item);
    }

    /// Returns the total number of items across all categories.
    pub fn total_items(&self) -> usize {
        self.key_decisions.len()
            + self.open_questions.len()
            + self.key_concepts.len()
            + self.technical_specifications.len()
            + self.conversation_flow.len()
    }

    /// Returns true if this context has no items.
    pub fn is_empty(&self) -> bool {
        self.total_items() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decision_item_confidence_clamping() {
        let item = DecisionItem::new(
            "Test".to_string(),
            "Context".to_string(),
            vec![],
            1.5, // Should be clamped to 1.0
            Utc::now(),
        );
        assert_eq!(item.confidence, 1.0);

        let item2 = DecisionItem::new(
            "Test".to_string(),
            "Context".to_string(),
            vec![],
            -0.5, // Should be clamped to 0.0
            Utc::now(),
        );
        assert_eq!(item2.confidence, 0.0);
    }

    #[test]
    fn test_flow_item_limit() {
        let mut ctx = StructuredContext::new();

        // Add MAX_FLOW_ITEMS + 100 items
        for i in 0..(MAX_FLOW_ITEMS + 100) {
            ctx.add_flow_item(FlowItem {
                step_description: format!("Step {}", i),
                ..Default::default()
            });
        }

        // Should have removed oldest 10% when limit was hit
        assert!(ctx.conversation_flow.len() <= MAX_FLOW_ITEMS);
    }

    #[test]
    fn test_structured_context_is_empty() {
        let ctx = StructuredContext::new();
        assert!(ctx.is_empty());
        assert_eq!(ctx.total_items(), 0);
    }

    #[test]
    fn test_default_implementations() {
        let _ = DecisionItem::default();
        let _ = QuestionItem::default();
        let _ = ConceptItem::default();
        let _ = SpecItem::default();
        let _ = FlowItem::default();
        let _ = QuestionStatus::default();
    }
}
