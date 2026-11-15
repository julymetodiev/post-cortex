// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The structured summary: the whole view, or the sections asked for.

use crate::capabilities::{StructuredSummary, SummarySection};
use post_cortex_core::summary::{
    DecisionSummary, EntitySummary, QuestionSummary, StructuredSummaryView,
};
use serde::Serialize;

/// A structured summary.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum SummaryReply {
    /// The whole summary.
    Full(Box<StructuredSummaryView>),
    /// Only the sections asked for (`include`).
    Sections(SummarySections),
}

/// The sections of a summary asked for, each with its data.
#[derive(Debug, Clone, Default, Serialize)]
pub struct SummarySections {
    /// `decisions`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_decisions: Option<Vec<DecisionSummary>>,
    /// `entities`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub important_entities: Option<Vec<String>>,
    /// `entities`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity_summaries: Option<Vec<EntitySummary>>,
    /// `questions` (the answered questions, named as stored).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open_questions: Option<Vec<QuestionSummary>>,
    /// `insights`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_insights: Option<Vec<String>>,
}

impl From<StructuredSummary> for SummaryReply {
    fn from(summary: StructuredSummary) -> Self {
        let Some(sections) = summary.sections else {
            return Self::Full(Box::new(summary.view));
        };
        let view = summary.view;
        let mut out = SummarySections::default();
        for section in sections {
            match section {
                SummarySection::Decisions => out.key_decisions = Some(view.key_decisions.clone()),
                SummarySection::Entities => {
                    out.important_entities = Some(view.important_entities.clone());
                    out.entity_summaries = Some(view.entity_summaries.clone());
                }
                SummarySection::Questions => {
                    out.open_questions = Some(view.answered_questions.clone());
                }
                SummarySection::Insights => {
                    out.key_insights = Some(summary.insights.clone().unwrap_or_default());
                }
            }
        }
        Self::Sections(out)
    }
}
