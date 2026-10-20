// Copyright (c) 2026 Julius ML
// MIT License

//! Scope resolution for `assemble_context`, shared by the MCP tool and the
//! gRPC `AssembleContext` RPC.
//!
//! Single-session scope borrows the session's entity graph (an `Arc` bump,
//! no deep clone). Workspace scope merges every member session's graph; the
//! merged graph is cached per workspace and reused for as long as every
//! member session still points at the same graph `Arc`.

use std::sync::{Arc, LazyLock};

use dashmap::DashMap;
use post_cortex_core::core::context_update::ContextUpdate;
use post_cortex_core::graph::entity_graph::SimpleEntityGraph;
use post_cortex_core::session::active_session::ActiveSession;
use post_cortex_embeddings::Corpus;
use post_cortex_memory::ConversationMemorySystem;
use post_cortex_memory::context_assembly::{self, AssembledContext};
use uuid::Uuid;

/// Updates visible to assembly for one session: hot context followed by the
/// warm tier.
pub(crate) fn session_updates(session: &ActiveSession, out: &mut Vec<ContextUpdate>) {
    // `HotContext::iter()` already returns an owned snapshot; move it in.
    out.extend(session.hot_context.iter());
    out.extend(session.warm_context.iter().map(|c| c.update.clone()));
}

/// A merged workspace graph plus the per-session graphs it was built from.
///
/// Holding the source `Arc`s is what makes pointer comparison sound: while
/// they are alive their addresses cannot be reused, and `Arc::make_mut` on a
/// session's graph is forced to copy instead of mutating in place.
/// Turns matched per assembly that covers messages (each comes with its
/// neighbouring turns).
const EXCERPT_MATCHES: usize = 5;

/// Assemble context for `query` from `corpus`: records (`updates` and
/// `graph`), conversation excerpts of `sessions`, or both. With both, the
/// records get half of `token_budget` and the excerpts what is left; with
/// messages only, the records are left out (entity context and impact
/// analysis stay). If the excerpt search fails the records stand alone.
pub(crate) async fn assemble(
    memory: &ConversationMemorySystem,
    query: &str,
    graph: &SimpleEntityGraph,
    updates: &[ContextUpdate],
    sessions: &[Uuid],
    token_budget: usize,
    corpus: Corpus,
) -> AssembledContext {
    let (updates, record_budget) = match corpus {
        Corpus::Records => (updates, token_budget),
        Corpus::All => (updates, token_budget / 2),
        Corpus::Messages => (&[][..], token_budget),
    };
    let mut ctx = context_assembly::assemble_context(query, graph, updates, record_budget);
    #[cfg(feature = "embeddings")]
    if corpus != Corpus::Records {
        match memory
            .conversation_excerpts(sessions, query, EXCERPT_MATCHES)
            .await
        {
            Ok(excerpts) => context_assembly::add_excerpts(&mut ctx, excerpts, token_budget),
            Err(e) => tracing::warn!("Conversation excerpts left out of assembled context: {e}"),
        }
    }
    #[cfg(not(feature = "embeddings"))]
    let _ = (memory, sessions);
    ctx
}

struct CachedMerge {
    sources: Vec<(Uuid, Arc<SimpleEntityGraph>)>,
    merged: Arc<SimpleEntityGraph>,
}

/// Merged-graph cache keyed by workspace id (one entry per workspace).
pub(crate) struct WorkspaceGraphCache {
    entries: DashMap<Uuid, CachedMerge>,
}

impl WorkspaceGraphCache {
    fn new() -> Self {
        Self {
            entries: DashMap::new(),
        }
    }

    /// Return the merged graph for `sources`, rebuilding it only when the set
    /// of (session id, graph `Arc`) pairs changed since the last call.
    pub(crate) fn merged(
        &self,
        workspace_id: Uuid,
        mut sources: Vec<(Uuid, Arc<SimpleEntityGraph>)>,
    ) -> Arc<SimpleEntityGraph> {
        // Workspace membership is a DashMap, so iteration order is not
        // stable; sort so both the cache key and the merge order are.
        sources.sort_unstable_by_key(|(id, _)| *id);

        if let Some(hit) = self.entries.get(&workspace_id)
            && hit.sources.len() == sources.len()
            && hit
                .sources
                .iter()
                .zip(&sources)
                .all(|((a_id, a), (b_id, b))| a_id == b_id && Arc::ptr_eq(a, b))
        {
            return Arc::clone(&hit.merged);
        }

        let mut merged = SimpleEntityGraph::new();
        for (_, graph) in &sources {
            merged.merge_from(graph);
        }
        let merged = Arc::new(merged);
        self.entries.insert(
            workspace_id,
            CachedMerge {
                sources,
                merged: Arc::clone(&merged),
            },
        );
        merged
    }
}

/// Process-wide cache shared by every MCP connection and the gRPC service.
pub(crate) static WORKSPACE_GRAPHS: LazyLock<WorkspaceGraphCache> =
    LazyLock::new(WorkspaceGraphCache::new);

#[cfg(test)]
mod tests {
    use super::*;
    use post_cortex_core::core::context_update::EntityType;

    fn graph_with(name: &str) -> Arc<SimpleEntityGraph> {
        let mut g = SimpleEntityGraph::new();
        g.add_or_update_entity(
            name.to_string(),
            EntityType::Concept,
            chrono::Utc::now(),
            "test",
        );
        Arc::new(g)
    }

    #[test]
    fn reuses_merge_while_session_graphs_are_unchanged() {
        let cache = WorkspaceGraphCache::new();
        let ws = Uuid::new_v4();
        let (s1, s2) = (Uuid::new_v4(), Uuid::new_v4());
        let (g1, g2) = (graph_with("a"), graph_with("b"));

        let first = cache.merged(ws, vec![(s1, g1.clone()), (s2, g2.clone())]);
        assert_eq!(first.entity_count(), 2);

        // Same graphs in a different order → cache hit.
        let second = cache.merged(ws, vec![(s2, g2.clone()), (s1, g1.clone())]);
        assert!(Arc::ptr_eq(&first, &second));

        // One session's graph replaced → rebuild.
        let third = cache.merged(ws, vec![(s1, g1), (s2, graph_with("c"))]);
        assert!(!Arc::ptr_eq(&first, &third));
        assert!(third.has_entity("c"));
        assert!(!third.has_entity("b"));
    }

    #[test]
    fn membership_change_invalidates() {
        let cache = WorkspaceGraphCache::new();
        let ws = Uuid::new_v4();
        let g1 = graph_with("a");
        let first = cache.merged(ws, vec![(Uuid::new_v4(), g1.clone())]);
        let second = cache.merged(ws, vec![(Uuid::new_v4(), g1)]);
        assert!(!Arc::ptr_eq(&first, &second));
    }
}
