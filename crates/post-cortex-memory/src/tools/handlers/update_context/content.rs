// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! The write tool's `content` map into a record's typed content.

use std::collections::HashMap;

use post_cortex_core::core::context_update::{UpdateContent, UpdateType};

use crate::tools::Failure;

/// Parse the LLM-provided `interaction_type + content` HashMap into a
/// typed [`UpdateContent`] by the tool's key conventions (`decision` and
/// `rationale` for a decision, ...). An unknown `interaction_type` is an
/// invalid argument.
pub fn build(
    interaction_type: &str,
    content: &HashMap<String, String>,
) -> Result<(UpdateType, UpdateContent), Failure> {
    let extract_extras = |exclude_keys: &[&str]| -> Vec<String> {
        content
            .iter()
            .filter(|(k, _)| !exclude_keys.contains(&k.as_str()))
            .map(|(k, v)| format!("{}: {}", k, v))
            .collect()
    };

    let resolve_slot = |preferred: &[&str], fallback_keys: &[&str]| -> String {
        for k in preferred.iter().chain(fallback_keys.iter()) {
            if let Some(v) = content.get(*k)
                && !v.trim().is_empty()
            {
                return v.clone();
            }
        }
        String::new()
    };

    let (update_type, title, description, details) = match interaction_type {
        "qa" => {
            let title = resolve_slot(&["question"], &["title"]);
            let description = resolve_slot(&["answer"], &["description"]);
            let details = extract_extras(&["question", "answer", "title", "description"]);
            (UpdateType::QuestionAnswered, title, description, details)
        }
        "code_change" => {
            let title = resolve_slot(&["file_path", "file"], &["title", "description"]);
            let description = resolve_slot(
                &["changes", "diff", "change_type", "change"],
                &["description"],
            );
            // The `file` key the title came from; the other, when both are
            // given, stays in the details, so no path it names is lost.
            let title_key = ["file_path", "file"]
                .into_iter()
                .find(|k| content.get(*k).is_some_and(|v| !v.trim().is_empty()));
            let used: Vec<&str> = title_key
                .into_iter()
                .chain([
                    "title",
                    "description",
                    "changes",
                    "diff",
                    "change_type",
                    "change",
                ])
                .collect();
            let details = extract_extras(&used);
            (UpdateType::CodeChanged, title, description, details)
        }
        "problem_solved" => {
            let title = resolve_slot(&["problem"], &["title"]);
            let description = resolve_slot(&["solution"], &["description"]);
            let details = extract_extras(&["problem", "solution", "title", "description"]);
            (UpdateType::ProblemSolved, title, description, details)
        }
        "decision_made" => {
            let title = resolve_slot(&["decision"], &["title"]);
            let description = resolve_slot(&["rationale"], &["description"]);
            let details = extract_extras(&["decision", "rationale", "title", "description"]);
            (UpdateType::DecisionMade, title, description, details)
        }
        "requirement_added" => {
            let title = resolve_slot(&["requirement"], &["title"]);
            let description = resolve_slot(&["description"], &[]);
            let details = extract_extras(&["requirement", "priority", "title", "description"]);
            (UpdateType::RequirementAdded, title, description, details)
        }
        "concept_defined" => {
            let title = resolve_slot(&["concept"], &["title"]);
            let description = resolve_slot(&["definition"], &["description"]);
            let details = extract_extras(&["concept", "definition", "title", "description"]);
            (UpdateType::ConceptDefined, title, description, details)
        }
        other => {
            return Err(
                Failure::invalid(format!("Unknown interaction type: {other}"))
                    .parameter("interaction_type")
                    .received(other),
            );
        }
    };

    Ok((
        update_type,
        UpdateContent {
            title,
            description,
            details,
            examples: vec![],
            implications: vec![],
        },
    ))
}
