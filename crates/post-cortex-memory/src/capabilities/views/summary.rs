// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The structured summary of a session: one operation for every transport,
//! each section with its own limit, the clock read once by the service.

use chrono::{DateTime, Utc};
use post_cortex_core::session::active_session::ActiveSession;
use post_cortex_core::summary::{StructuredSummaryView, SummaryGenerator, SummaryOptions};
use uuid::Uuid;

use crate::capabilities::views::ViewsError;

/// A summary larger than this (estimated) is compacted unless the caller
/// asked for compact output already.
pub const MAX_SUMMARY_TOKENS: usize = 50_000;

/// Insights returned when the request gives no limit.
pub const DEFAULT_INSIGHTS_LIMIT: usize = 5;

/// A section of the structured summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SummarySection {
    /// `key_decisions`.
    Decisions,
    /// `key_insights`.
    Insights,
    /// `important_entities` and `entity_summaries`.
    Entities,
    /// `open_questions`.
    Questions,
}

impl SummarySection {
    /// The wire name of the section.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Decisions => "decisions",
            Self::Insights => "insights",
            Self::Entities => "entities",
            Self::Questions => "questions",
        }
    }

    /// The sections an `include` list names, in the fixed order decisions,
    /// entities, questions, insights; None (every section, the whole
    /// summary) when it is absent or names `all`. Names are
    /// case-insensitive; unknown names are skipped.
    ///
    /// # Errors
    /// `InvalidArgument` when the list names no known section.
    pub fn parse_include(include: Option<&[String]>) -> Result<Option<Vec<Self>>, ViewsError> {
        let Some(given) = include else {
            return Ok(None);
        };
        let names: Vec<String> = given.iter().map(|s| s.to_lowercase()).collect();
        if names.iter().any(|n| n == "all") {
            return Ok(None);
        }
        let sections: Vec<Self> = [
            Self::Decisions,
            Self::Entities,
            Self::Questions,
            Self::Insights,
        ]
        .into_iter()
        .filter(|s| names.iter().any(|n| n == s.as_str()))
        .collect();
        if sections.is_empty() {
            return Err(ViewsError::InvalidArgument(format!(
                "include: no known section in {given:?}; valid: decisions, insights, entities, questions, all"
            )));
        }
        Ok(Some(sections))
    }
}

/// What summary to build.
#[derive(Debug, Clone, Default)]
pub struct SummaryRequest {
    /// The session.
    pub session_id: Uuid,
    /// The sections to return; None returns the whole summary.
    pub sections: Option<Vec<SummarySection>>,
    /// Most decisions.
    pub decisions_limit: Option<usize>,
    /// Most entities.
    pub entities_limit: Option<usize>,
    /// Most open questions.
    pub questions_limit: Option<usize>,
    /// Most concepts.
    pub concepts_limit: Option<usize>,
    /// Most insights ([`DEFAULT_INSIGHTS_LIMIT`] when None).
    pub insights_limit: Option<usize>,
    /// Decisions below this confidence are left out.
    pub min_confidence: Option<f32>,
    /// Compact output (fixed small limits).
    pub compact: bool,
}

/// How the summary was limited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compaction {
    /// The request's limits applied.
    None,
    /// The caller asked for compact output.
    Requested,
    /// The summary was estimated larger than [`MAX_SUMMARY_TOKENS`] and was
    /// compacted.
    Auto {
        /// The estimate.
        estimated_tokens: usize,
    },
}

/// A structured summary.
#[derive(Debug, Clone)]
pub struct StructuredSummary {
    /// The summary; with `sections` set, only those sections are meant to
    /// be shown.
    pub view: StructuredSummaryView,
    /// The sections asked for (None: all of them).
    pub sections: Option<Vec<SummarySection>>,
    /// The key insights, when the insights section was asked for.
    pub insights: Option<Vec<String>>,
    /// How the summary was limited.
    pub compaction: Compaction,
}

/// Build the summary of `session` as of `now`.
pub fn build(
    session: &ActiveSession,
    req: SummaryRequest,
    now: DateTime<Utc>,
) -> StructuredSummary {
    let (estimated_tokens, too_big) =
        SummaryGenerator::estimate_summary_size(session, MAX_SUMMARY_TOKENS);
    let compaction = if req.compact {
        Compaction::Requested
    } else if too_big {
        tracing::info!(
            "Pre-estimated {} tokens > {} max. Using compact mode directly.",
            estimated_tokens,
            MAX_SUMMARY_TOKENS
        );
        Compaction::Auto { estimated_tokens }
    } else {
        Compaction::None
    };
    let options = match compaction {
        Compaction::None => SummaryOptions {
            decisions_limit: req.decisions_limit,
            entities_limit: req.entities_limit,
            questions_limit: req.questions_limit,
            concepts_limit: req.concepts_limit,
            min_confidence: req.min_confidence,
            compact: false,
        },
        _ => SummaryOptions::compact(),
    };
    let view = SummaryGenerator::generate_structured_summary_filtered(session, &options, now);
    let insights = req
        .sections
        .as_ref()
        .filter(|s| s.contains(&SummarySection::Insights))
        .map(|_| {
            SummaryGenerator::extract_key_insights(
                session,
                req.insights_limit.unwrap_or(DEFAULT_INSIGHTS_LIMIT),
            )
        });
    StructuredSummary {
        view,
        sections: req.sections,
        insights,
        compaction,
    }
}
