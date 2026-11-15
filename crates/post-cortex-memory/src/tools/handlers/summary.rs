// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! Tool `get_structured_summary`: the whole summary or the sections asked
//! for (`include`), each section with its own limit.

use crate::Memory;
use crate::capabilities::{Compaction, StructuredSummary, SummaryRequest, SummarySection};
use serde_json::Value;

use crate::tools::Failure;
use crate::tools::args::GetStructuredSummaryRequest;
use crate::tools::check;
use crate::tools::handlers::Outcome;
use crate::tools::parse::{FieldHint, arguments};
use crate::tools::reply;
use crate::tools::responses::SummaryReply;

const HINTS: &[FieldHint] = &[
    FieldHint {
        field: "session_id",
        expected: "UUID string (36 chars with hyphens)",
        hint: "Provide a valid session UUID to get its summary",
    },
    FieldHint {
        field: "include",
        expected: "array of: decisions, insights, entities, questions, all",
        hint: "Specify which sections to include, or omit for all. Valid values: decisions, insights, entities, questions, all",
    },
];

pub async fn handle(memory: &Memory, params: Value) -> Outcome {
    let req: GetStructuredSummaryRequest = arguments(params, HINTS)?;
    let session_id = check::session_id(&req.session_id)?;
    let decisions_limit = check::limit("decisions_limit", req.decisions_limit, 10, 100)?;
    let entities_limit = check::limit("entities_limit", req.entities_limit, 20, 200)?;
    let questions_limit = check::limit("questions_limit", req.questions_limit, 5, 50)?;
    let concepts_limit = check::limit("concepts_limit", req.concepts_limit, 10, 50)?;
    let sections = SummarySection::parse_include(req.include.as_deref())
        .map_err(|e| Failure::from(e).parameter("include"))?;
    // The one summary operation; every section has its own limit.
    let summary = memory
        .views
        .summary(SummaryRequest {
            session_id,
            sections,
            decisions_limit: Some(decisions_limit),
            entities_limit: Some(entities_limit),
            questions_limit: Some(questions_limit),
            concepts_limit: Some(concepts_limit),
            insights_limit: None,
            min_confidence: req.min_confidence,
            compact: req.compact.unwrap_or(false),
        })
        .await
        .map_err(|e| {
            let message = format!("Failed to load session: {e}");
            Failure::from(e).with_message(message)
        })?;
    let message = message(&summary);
    reply::with(message, &SummaryReply::from(summary))
}

/// The text of a summary reply.
fn message(summary: &StructuredSummary) -> String {
    let view = &summary.view;
    if let Some(sections) = &summary.sections {
        let named: Vec<&str> = sections.iter().map(|s| s.as_str()).collect();
        return format!("Structured summary sections: {}", named.join(", "));
    }
    match summary.compaction {
        Compaction::Requested => "Generated compact structured summary".to_string(),
        Compaction::Auto { .. } => format!(
            "Auto-compacted summary (was too large for MCP). Showing: {} decisions, {} entities, {} questions, {} concepts",
            view.key_decisions.len(),
            view.entity_summaries.len(),
            view.answered_questions.len(),
            view.key_concepts.len()
        ),
        Compaction::None => format!(
            "Generated structured summary (decisions: {}, entities: {}, questions: {}, concepts: {})",
            view.key_decisions.len(),
            view.entity_summaries.len(),
            view.answered_questions.len(),
            view.key_concepts.len()
        ),
    }
}

#[cfg(test)]
mod tests;
