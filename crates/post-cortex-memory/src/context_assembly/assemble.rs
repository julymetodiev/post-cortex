// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Packing the graph around the query, the records search found, the cards
//! and the conversation excerpts into the token budget.

use crate::context_assembly::format::estimate_tokens;
use crate::context_assembly::graph::GraphContext;
use crate::context_assembly::{AssembledContext, ContextItem, ContextSource};
use post_cortex_core::core::card::Card;
use tracing::debug;

/// The graph `around` the query and the `records` (best first, as search
/// ranked them) within `token_budget`: the records fill what the graph
/// leaves, in their order; a record that does not fit is skipped. Each
/// record names the entities of the graph context its text mentions.
pub fn assemble_context(
    around: GraphContext,
    records: Vec<ContextItem>,
    token_budget: usize,
) -> AssembledContext {
    let GraphContext {
        entity_context,
        impact,
        tokens: graph_tokens,
    } = around;
    let names: Vec<(String, &str)> = entity_context
        .iter()
        .map(|ec| (ec.name.to_lowercase(), ec.name.as_str()))
        .collect();
    let room = token_budget.saturating_sub(graph_tokens);
    let mut used = 0;
    let mut items = Vec::new();
    for mut item in records {
        if used + item.token_estimate > room {
            continue;
        }
        let text = item.text.to_lowercase();
        item.entities = names
            .iter()
            .filter(|(lower, _)| text.contains(lower.as_str()))
            .map(|(_, name)| (*name).to_string())
            .collect();
        used += item.token_estimate;
        items.push(item);
    }
    debug!(
        "Assembled {} records ({} tokens), {} entity contexts, {} impact entries",
        items.len(),
        used,
        entity_context.len(),
        impact.len()
    );
    AssembledContext {
        items,
        entity_context,
        impact,
        total_tokens: used + graph_tokens,
        cards: Vec::new(),
    }
}

/// A record (or document) search found, for [`assemble_context`].
pub fn record(text: String, score: f32, entry_id: String) -> ContextItem {
    ContextItem {
        token_estimate: estimate_tokens(&text),
        text,
        score,
        source: ContextSource::SemanticMatch,
        entities: Vec::new(),
        entry_id,
    }
}

/// Tokens the non-empty `cards` take in front of a context.
pub fn card_tokens(cards: &[Card]) -> usize {
    cards
        .iter()
        .filter(|c| !c.text.trim().is_empty())
        .map(|c| estimate_tokens(&c.text) + 8)
        .sum()
}

/// Put the user and project `cards` in front of `ctx` (empty ones are
/// left out). They always go in; callers keep [`card_tokens`] free in the
/// budget they give the rest.
pub fn add_cards(ctx: &mut AssembledContext, cards: Vec<Card>) {
    let cards: Vec<Card> = cards
        .into_iter()
        .filter(|c| !c.text.trim().is_empty())
        .collect();
    ctx.total_tokens += card_tokens(&cards);
    ctx.cards = cards;
}

/// Add conversation excerpts (see [`ContextSource::Conversation`]) to `ctx`
/// in their order, as far as `token_budget` allows: an excerpt that does not
/// fit is skipped.
pub fn add_excerpts(ctx: &mut AssembledContext, excerpts: Vec<ContextItem>, token_budget: usize) {
    for item in excerpts {
        if ctx.total_tokens + item.token_estimate > token_budget {
            continue;
        }
        ctx.total_tokens += item.token_estimate;
        ctx.items.push(item);
    }
}

/// A conversation excerpt for [`add_excerpts`].
pub fn excerpt(text: String, score: f32, entry_id: String) -> ContextItem {
    ContextItem {
        token_estimate: estimate_tokens(&text),
        text,
        score,
        source: ContextSource::Conversation,
        entities: Vec::new(),
        entry_id,
    }
}
