// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The assembled context.

use crate::context_assembly::{
    AssembledContext, ContextItem, EntityContext, ImpactEntry, format_for_llm,
};
use post_cortex_core::core::card::Card;
use serde::Serialize;

/// Context assembled for a query: what MCP `assemble_context` returns and
/// what `select` (MCP and gRPC) runs over.
#[derive(Debug, Clone, Serialize)]
pub struct Assembled {
    /// The query.
    pub query: String,
    /// The token budget it was packed into.
    pub token_budget: usize,
    /// Tokens used.
    pub total_tokens: usize,
    /// Ranked items.
    pub items: Vec<ContextItem>,
    /// The entities matched and their neighbours.
    pub entity_context: Vec<EntityContext>,
    /// Who depends on them.
    pub impact: Vec<ImpactEntry>,
    /// The cards put in front.
    pub cards: Vec<Card>,
    /// The LLM-ready block.
    pub formatted_text: String,
}

impl Assembled {
    /// The response of `context` assembled for `query` within `token_budget`.
    #[must_use]
    pub fn new(query: String, token_budget: usize, context: AssembledContext) -> Self {
        let formatted_text = format_for_llm(&context);
        Self {
            query,
            token_budget,
            total_tokens: context.total_tokens,
            items: context.items,
            entity_context: context.entity_context,
            impact: context.impact,
            cards: context.cards,
            formatted_text,
        }
    }
}
