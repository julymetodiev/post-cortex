// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Assembly: graph-aware context for a query over a session or a
//! workspace (ARCHITECTURE section 4, docs/memory.md): the cards first,
//! then the graph around the query, the records Search ranks for it and
//! conversation excerpts, within a token budget. MCP `assemble_context`, its
//! `query_conversation_context` alias and gRPC `AssembleContext` all call
//! [`Assembly::assemble`].

use std::sync::Arc;

use post_cortex_core::graph::entity_graph::SimpleEntityGraph;
use post_cortex_ports::Corpus;
use uuid::Uuid;

use crate::capabilities::cards::Cards;
use crate::capabilities::scope::{self, ScopeError, Target};
use crate::capabilities::search::Search;
use crate::capabilities::search::SearchScope;
use crate::content_vectorizer::{ContentKind, SearchOptions};
use crate::context_assembly::{self, AssembledContext, ContextItem};
use crate::memory_system::MemoryRuntime;

/// Token budget when the request gives none.
pub const DEFAULT_TOKEN_BUDGET: usize = 4000;

/// Turns matched per assembly that covers messages (each comes with its
/// neighbouring turns).
const EXCERPT_MATCHES: usize = 5;

/// Records Search ranks per assembly; the budget decides how many go in.
const RECORD_MATCHES: usize = 20;

/// Characters of a record shown: its passage for the query, so one long
/// record cannot take the room of the others (~200 tokens).
const RECORD_CHARS: usize = 800;

/// What to assemble.
#[derive(Debug, Clone)]
pub struct AssembleRequest {
    /// The question or topic.
    pub query: String,
    /// The session or workspace to read.
    pub target: Target,
    /// Tokens the result may take ([`DEFAULT_TOKEN_BUDGET`] when 0).
    pub token_budget: usize,
    /// Records, messages, or both.
    pub corpus: Corpus,
    /// The person it is for, whose user card comes first; none: the host's.
    pub person: Option<Uuid>,
}

/// Why an assembly failed.
#[derive(Debug, thiserror::Error)]
pub enum AssemblyError {
    /// The request is malformed (an empty query).
    #[error("Invalid argument: {0}")]
    InvalidArgument(String),
    /// The scope could not be resolved.
    #[error(transparent)]
    Scope(#[from] ScopeError),
}

/// The Assembly capability service.
#[derive(Clone)]
pub struct Assembly {
    rt: Arc<MemoryRuntime>,
    cards: Cards,
    search: Search,
}

/// What an assembly draws on: `graph`, the records found in `search`, the
/// excerpts of `sessions` and the cards of `workspaces`.
pub struct AssemblyScope<'a> {
    /// The scope's graph.
    pub graph: &'a SimpleEntityGraph,
    /// Where the records are searched.
    pub search: SearchScope,
    /// The sessions whose messages give excerpts.
    pub sessions: &'a [Uuid],
    /// The workspaces whose cards come first.
    pub workspaces: &'a [Uuid],
    /// The person whose user card comes first; none: the host's.
    pub person: Option<Uuid>,
}

impl Assembly {
    pub(crate) fn new(rt: Arc<MemoryRuntime>, cards: Cards, search: Search) -> Self {
        Self { rt, cards, search }
    }

    /// Assemble context for `req.query`: the user card and the cards of the
    /// scope's workspaces first (always), then the graph around the
    /// entities the query names (at most half of what the cards leave),
    /// then from the records Search ranks for the query, conversation
    /// excerpts of the sessions, or both. With both, the excerpts take at
    /// most half of what the cards and the graph leave and the records the
    /// rest (all of it when no turn matches); with messages only the
    /// records are left out (the graph stays). The whole stays within the
    /// budget unless the cards alone exceed it. If a search fails (or
    /// embeddings are off) its part is left out and the rest stands.
    ///
    /// # Errors
    /// `InvalidArgument` for an empty (or blank) query; the scope could not
    /// be resolved.
    pub async fn assemble(&self, req: AssembleRequest) -> Result<AssembledContext, AssemblyError> {
        if req.query.trim().is_empty() {
            return Err(AssemblyError::InvalidArgument(
                "query cannot be empty".to_string(),
            ));
        }
        let search = match req.target {
            Target::Session(id) => SearchScope::Session(id),
            Target::Workspace(id) => SearchScope::Workspace(id),
            Target::Global => SearchScope::Global,
        };
        let resolved = scope::resolve(&self.rt, req.target).await?;
        let token_budget = match req.token_budget {
            0 => DEFAULT_TOKEN_BUDGET,
            n => n,
        };
        let scope = AssemblyScope {
            graph: &resolved.scope.graph,
            search,
            sessions: &resolved.session_ids(),
            workspaces: &resolved.workspaces,
            person: req.person,
        };
        Ok(self
            .assemble_over(&req.query, scope, token_budget, req.corpus)
            .await)
    }

    /// [`Self::assemble`] over a resolved scope.
    pub(crate) async fn assemble_over(
        &self,
        query: &str,
        scope: AssemblyScope<'_>,
        token_budget: usize,
        corpus: Corpus,
    ) -> AssembledContext {
        let AssemblyScope {
            graph,
            search,
            sessions,
            workspaces,
            person,
        } = scope;
        let cards = self.cards.for_assembly(person, workspaces).await;
        let token_budget = token_budget.saturating_sub(context_assembly::card_tokens(&cards));
        let around = context_assembly::graph_context(query, graph, token_budget / 2);
        let left = token_budget - around.tokens;
        let excerpts = match corpus {
            Corpus::Records => Vec::new(),
            _ => self.excerpts(sessions, query).await,
        };
        // Excerpts get half of what the graph leaves when records come too.
        let (records, excerpt_budget) = match corpus {
            Corpus::Messages => (Vec::new(), left),
            _ => (self.records(search, query).await, left / 2),
        };
        // The excerpts that fit their share, in conversation order.
        let mut excerpt_tokens = 0;
        let excerpts: Vec<_> = excerpts
            .into_iter()
            .filter(|item| {
                let fits = excerpt_tokens + item.token_estimate <= excerpt_budget;
                if fits {
                    excerpt_tokens += item.token_estimate;
                }
                fits
            })
            .collect();
        let mut ctx =
            context_assembly::assemble_context(around, records, token_budget - excerpt_tokens);
        context_assembly::add_excerpts(&mut ctx, excerpts, token_budget);
        context_assembly::add_cards(&mut ctx, cards);
        ctx
    }

    /// The records (and, in a workspace, documents) Search ranks for
    /// `query` in `scope`, best first, each shown as its passage for the
    /// query ([`RECORD_CHARS`]); records that no longer hold and entity
    /// descriptions (the graph context has the entities) are left out. If
    /// the search fails the records are left out.
    async fn records(&self, scope: SearchScope, query: &str) -> Vec<ContextItem> {
        let options = SearchOptions {
            limit: Some(RECORD_MATCHES),
            corpus: Corpus::Records,
            passage: Some(RECORD_CHARS),
            without_neighbours: true,
            ..Default::default()
        };
        let hits = match self.search.search(scope, query, options).await {
            Ok(hits) => hits,
            Err(e) => {
                tracing::warn!("Records left out of assembled context: {e}");
                return Vec::new();
            }
        };
        hits.into_iter()
            .filter(|hit| hit.content_type != ContentKind::EntityDescription)
            .filter(|hit| hit.validity.as_ref().is_none_or(|v| v.is_current()))
            .map(|hit| {
                // A title cut from the text's start ("..." at its end) is
                // not repeated in front of it.
                let text = match &hit.title {
                    Some(title)
                        if !hit
                            .text_content
                            .starts_with(title.trim_end_matches("...").trim_end()) =>
                    {
                        format!("{title}: {}", hit.text_content)
                    }
                    _ => hit.text_content,
                };
                context_assembly::record(text, hit.combined_score.clamp(0.0, 1.0), hit.content_id)
            })
            .collect()
    }

    /// The captured turns of `sessions` that best match `query`, each with
    /// the turns around it, as context excerpts in conversation order
    /// (`[date] role: text`), at most [`EXCERPT_MATCHES`] matches. A fast
    /// search: no reranking. If it fails the excerpts are left out.
    async fn excerpts(&self, sessions: &[Uuid], query: &str) -> Vec<ContextItem> {
        use crate::content_vectorizer::SearchMode;
        use post_cortex_core::core::message;

        let options = SearchOptions {
            limit: Some(EXCERPT_MATCHES),
            mode: Some(SearchMode::Fast),
            corpus: Corpus::Messages,
            ..Default::default()
        };
        let scope = match sessions {
            [] => return Vec::new(),
            [session_id] => SearchScope::Session(*session_id),
            sessions => SearchScope::Sessions(sessions.to_vec()),
        };
        let results = match self.search.search(scope, query, options).await {
            Ok(results) => results,
            Err(e) => {
                tracing::warn!("Conversation excerpts left out of assembled context: {e}");
                return Vec::new();
            }
        };
        // Group each match with its neighbours, in stream order.
        let mut turns: Vec<_> = results
            .into_iter()
            .filter(|r| r.content_type == ContentKind::Message)
            .collect();
        // Turns live in sessions: each source is a session.
        turns.sort_by_key(|r| {
            (
                r.source.id(),
                message::seq_of_content_id(&r.content_id).unwrap_or(u64::MAX),
            )
        });
        turns
            .into_iter()
            .map(|r| {
                context_assembly::excerpt(
                    format!(
                        "[{}] {}",
                        r.timestamp.format("%Y-%m-%d %H:%M"),
                        r.text_content
                    ),
                    r.combined_score.clamp(0.0, 1.0),
                    format!("{}/{}", r.source.id(), r.content_id),
                )
            })
            .collect()
    }
}

#[cfg(test)]
mod tests;
