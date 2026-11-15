// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The assembled context as text for an LLM, and its token estimates.

use crate::context_assembly::{
    AssembledContext, ContextItem, ContextSource, EntityContext, ImpactEntry,
};
use post_cortex_core::core::card::CardScope;

/// Tokens the entity-relationship and impact sections take in
/// [`format_for_llm`]'s text.
pub fn graph_section_tokens(entity_context: &[EntityContext], impact: &[ImpactEntry]) -> usize {
    [entity_section(entity_context), impact_section(impact)]
        .iter()
        .flatten()
        .map(|section| estimate_tokens(section))
        .sum()
}

/// The deduplicated relationship lines of the entity context, if any.
fn entity_section(entity_context: &[EntityContext]) -> Option<String> {
    let mut graph_lines: Vec<String> = entity_context
        .iter()
        .flat_map(|ec| &ec.relationships)
        .map(|rel| {
            format!(
                "  {} --[{}]--> {}",
                rel.from_entity,
                rel.relation_type.stored_name(),
                rel.to_entity
            )
        })
        .collect();
    if graph_lines.is_empty() {
        return None;
    }
    graph_lines.sort();
    graph_lines.dedup();
    Some(format!("Entity relationships:\n{}", graph_lines.join("\n")))
}

/// The impact-analysis lines, if any.
fn impact_section(impact: &[ImpactEntry]) -> Option<String> {
    if impact.is_empty() {
        return None;
    }
    let impact_lines: Vec<String> = impact
        .iter()
        .map(|i| {
            format!(
                "  {} depends on {} ({})",
                i.entity,
                i.depends_on,
                i.relation_type.stored_name()
            )
        })
        .collect();
    Some(format!(
        "Impact analysis — these entities depend on what you're working with:\n{}",
        impact_lines.join("\n")
    ))
}

/// Rough token estimate: ~4 chars per token for English
pub fn estimate_tokens(text: &str) -> usize {
    text.len().div_ceil(4)
}

/// Format assembled context as a text block suitable for LLM injection.
pub fn format_for_llm(ctx: &AssembledContext) -> String {
    let mut parts: Vec<String> = Vec::new();

    // Cards first: the standing context of the user and the project.
    for card in &ctx.cards {
        let title = match card.scope {
            CardScope::User | CardScope::Person(_) => "User card",
            CardScope::Workspace(_) => "Project card",
        };
        parts.push(format!("{title}:\n{}", card.text.trim()));
    }

    // Entity graph summary and impact warnings
    parts.extend(entity_section(&ctx.entity_context));
    parts.extend(impact_section(&ctx.impact));

    // Context items: records, then conversation excerpts
    let (excerpts, records): (Vec<&ContextItem>, Vec<&ContextItem>) = ctx
        .items
        .iter()
        .partition(|item| item.source == ContextSource::Conversation);
    if !records.is_empty() {
        let content_lines: Vec<&str> = records.iter().map(|item| item.text.as_str()).collect();
        parts.push(format!(
            "Relevant context:\n{}",
            content_lines.join("\n---\n")
        ));
    }
    if !excerpts.is_empty() {
        let lines: Vec<&str> = excerpts.iter().map(|item| item.text.as_str()).collect();
        parts.push(format!("Conversation excerpts:\n{}", lines.join("\n")));
    }

    parts.join("\n\n")
}
