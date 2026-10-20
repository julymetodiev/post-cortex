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
use post_cortex_core::core::card::{Card, CardScope};
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

/// The workspaces `session_id` belongs to.
pub(crate) fn workspaces_of(memory: &ConversationMemorySystem, session_id: Uuid) -> Vec<Uuid> {
    memory
        .workspace_manager
        .list_workspaces()
        .iter()
        .filter(|ws| {
            ws.get_all_sessions()
                .iter()
                .any(|(id, _)| *id == session_id)
        })
        .map(|ws| ws.id)
        .collect()
}

/// The user card and the cards of `workspaces`, in that order; cards that
/// cannot be read are left out.
async fn cards(memory: &ConversationMemorySystem, workspaces: &[Uuid]) -> Vec<Card> {
    let scopes = std::iter::once(CardScope::User)
        .chain(workspaces.iter().copied().map(CardScope::Workspace));
    let mut cards = Vec::new();
    for scope in scopes {
        match memory.storage_actor.get_card(scope).await {
            Ok(Some(card)) => cards.push(card),
            Ok(None) => {}
            Err(e) => tracing::warn!("Card {} left out of assembled context: {e}", scope.key()),
        }
    }
    cards
}

/// Assemble context for `query`: the user card and the cards of
/// `workspaces` first (always, see ROADMAP Phase 2 item 3), then from
/// `corpus` records (`updates` and `graph`), conversation excerpts of
/// `sessions`, or both. With both, the excerpts take at most half of what
/// the cards leave of `token_budget` and the records the rest (all of it
/// when no turn matches); with messages only, the records are left out
/// (entity context and impact analysis stay). If the excerpt search fails
/// the records stand alone.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn assemble(
    memory: &ConversationMemorySystem,
    query: &str,
    graph: &SimpleEntityGraph,
    updates: &[ContextUpdate],
    sessions: &[Uuid],
    workspaces: &[Uuid],
    token_budget: usize,
    corpus: Corpus,
) -> AssembledContext {
    let cards = cards(memory, workspaces).await;
    let token_budget = token_budget.saturating_sub(context_assembly::card_tokens(&cards));
    let excerpts = match corpus {
        Corpus::Records => Vec::new(),
        _ => conversation_excerpts(memory, sessions, query).await,
    };
    // Excerpts get half of what the entity context leaves, so large
    // entity contexts cannot squeeze the records out.
    let (updates, excerpt_budget) = match corpus {
        Corpus::Messages => (&[][..], token_budget),
        _ => (
            updates,
            token_budget.saturating_sub(context_assembly::entity_summary_tokens(query, graph)) / 2,
        ),
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
        context_assembly::assemble_context(query, graph, updates, token_budget - excerpt_tokens);
    context_assembly::add_excerpts(&mut ctx, excerpts, token_budget);
    context_assembly::add_cards(&mut ctx, cards);
    ctx
}

#[cfg(feature = "embeddings")]
async fn conversation_excerpts(
    memory: &ConversationMemorySystem,
    sessions: &[Uuid],
    query: &str,
) -> Vec<context_assembly::ContextItem> {
    memory
        .conversation_excerpts(sessions, query, EXCERPT_MATCHES)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!("Conversation excerpts left out of assembled context: {e}");
            Vec::new()
        })
}

#[cfg(not(feature = "embeddings"))]
async fn conversation_excerpts(
    _memory: &ConversationMemorySystem,
    _sessions: &[Uuid],
    _query: &str,
) -> Vec<context_assembly::ContextItem> {
    Vec::new()
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

    /// With records and messages, the excerpts take at most half the budget
    /// and the records the rest; with no matching turn the records get all
    /// of it (they used to get half regardless).
    #[cfg(feature = "embeddings")]
    #[tokio::test]
    async fn all_sources_share_the_budget() {
        use post_cortex_core::core::message::{MessageRole, NewMessage};
        use post_cortex_memory::context_assembly::ContextSource;

        let dir = tempfile::tempdir().unwrap();
        let memory = ConversationMemorySystem::new(post_cortex_memory::SystemConfig {
            data_directory: dir.path().to_str().unwrap().to_string(),
            ..Default::default()
        })
        .await
        .unwrap();
        let sid = memory.create_session(None, None).await.unwrap();
        for i in 0..6 {
            let text = format!("zebracorn rollout note {i}: {}", "details ".repeat(60));
            memory
                .add_incremental_update(sid, text, None)
                .await
                .unwrap();
        }
        let assemble_now = |corpus| {
            let memory = &memory;
            async move {
                let session = memory.get_session(sid).await.unwrap().load_full();
                let mut updates = Vec::new();
                session_updates(&session, &mut updates);
                assemble(
                    memory,
                    "zebracorn rollout",
                    &session.entity_graph,
                    &updates,
                    &[sid],
                    &[],
                    600,
                    corpus,
                )
                .await
            }
        };
        let count = |ctx: &AssembledContext, conversation: bool| {
            ctx.items
                .iter()
                .filter(|i| (i.source == ContextSource::Conversation) == conversation)
                .count()
        };

        let records = assemble_now(Corpus::Records).await;
        let all_without_turns = assemble_now(Corpus::All).await;
        assert!(count(&records, false) >= 2, "{records:?}");
        assert_eq!(count(&all_without_turns, false), count(&records, false));

        let turn = |id: &str, text: &str| NewMessage {
            turn_id: id.to_string(),
            role: MessageRole::User,
            author: None,
            source_session: None,
            timestamp: chrono::Utc::now(),
            text: text.to_string(),
            tools: Vec::new(),
            files: Vec::new(),
        };
        memory
            .append_messages(
                sid,
                vec![
                    turn("t1", "when does the zebracorn rollout start?"),
                    turn("t2", "the zebracorn rollout starts on monday"),
                ],
            )
            .await
            .unwrap();
        let all = assemble_now(Corpus::All).await;
        assert!(count(&all, true) >= 1, "{all:?}");
        assert!(count(&all, false) >= 1, "records keep the rest: {all:?}");
        assert!(all.total_tokens <= 600, "{}", all.total_tokens);

        let messages = assemble_now(Corpus::Messages).await;
        assert_eq!(count(&messages, false), 0);
        assert!(count(&messages, true) >= 1);
    }

    /// The user card and the cards of the session's workspaces come first,
    /// in the payload and the formatted text, and count against the budget.
    #[tokio::test]
    async fn cards_come_first() {
        use post_cortex_core::workspace::SessionRole;

        let dir = tempfile::tempdir().unwrap();
        let memory = ConversationMemorySystem::new(post_cortex_memory::SystemConfig {
            data_directory: dir.path().to_str().unwrap().to_string(),
            enable_embeddings: false,
            ..Default::default()
        })
        .await
        .unwrap();
        let sid = memory.create_session(None, None).await.unwrap();
        memory
            .add_incremental_update(sid, "zebracorn rollout plan".to_string(), None)
            .await
            .unwrap();
        let ws = memory
            .workspace_manager
            .create_workspace("p".to_string(), String::new());
        memory
            .workspace_manager
            .add_session_to_workspace(&ws, sid, SessionRole::Primary)
            .unwrap();
        let other = memory
            .workspace_manager
            .create_workspace("other".to_string(), String::new());
        for (scope, text) in [
            (CardScope::User, "Answer briefly."),
            (CardScope::Workspace(ws), "## Stack\nrust"),
            (CardScope::Workspace(other), "not this project"),
        ] {
            memory
                .storage_actor
                .put_card(scope, text.to_string(), None, 0)
                .await
                .unwrap();
        }

        let workspaces = workspaces_of(&memory, sid);
        assert_eq!(workspaces, [ws]);
        let session = memory.get_session(sid).await.unwrap().load_full();
        let mut updates = Vec::new();
        session_updates(&session, &mut updates);
        let ctx = assemble(
            &memory,
            "zebracorn",
            &session.entity_graph,
            &updates,
            &[sid],
            &workspaces,
            4000,
            Corpus::Records,
        )
        .await;
        let texts: Vec<&str> = ctx.cards.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(texts, ["Answer briefly.", "## Stack\nrust"]);
        let formatted = context_assembly::format_for_llm(&ctx);
        assert!(
            formatted.starts_with("User card:\nAnswer briefly.\n\nProject card:\n## Stack\nrust"),
            "{formatted}"
        );
        assert!(formatted.contains("zebracorn rollout plan"));
        assert!(ctx.total_tokens >= context_assembly::card_tokens(&ctx.cards));
    }

    /// A query whose entity context has many relationships reserves much of
    /// the budget for them; the excerpts get half of what is left, so records
    /// still fit (live on ml the reserve took 1850 of 3000 tokens and the
    /// records got nothing).
    #[cfg(feature = "embeddings")]
    #[tokio::test]
    async fn large_entity_context_leaves_records_room() {
        use post_cortex_core::core::context_update::{
            EntityRelationship, EntityType, RelationType,
        };
        use post_cortex_core::core::message::{MessageRole, NewMessage};
        use post_cortex_memory::context_assembly::ContextSource;

        let dir = tempfile::tempdir().unwrap();
        let memory = ConversationMemorySystem::new(post_cortex_memory::SystemConfig {
            data_directory: dir.path().to_str().unwrap().to_string(),
            ..Default::default()
        })
        .await
        .unwrap();
        let sid = memory.create_session(None, None).await.unwrap();
        for i in 0..4 {
            memory
                .add_incremental_update(sid, format!("zebracorn rollout record {i}"), None)
                .await
                .unwrap();
        }
        let long = "zebracorn rollout discussion ".repeat(45);
        memory
            .append_messages(
                sid,
                (0..4)
                    .map(|i| NewMessage {
                        turn_id: format!("t{i}"),
                        role: MessageRole::User,
                        author: None,
                        source_session: None,
                        timestamp: chrono::Utc::now(),
                        text: format!("{long} {i}"),
                        tools: Vec::new(),
                        files: Vec::new(),
                    })
                    .collect(),
            )
            .await
            .unwrap();

        let mut graph = SimpleEntityGraph::new();
        let names: Vec<String> = (0..30).map(|i| format!("component{i}")).collect();
        for name in &names {
            graph.add_or_update_entity(name.clone(), EntityType::Concept, chrono::Utc::now(), "t");
        }
        // Four relations each: the relationships section takes ~1,300 tokens.
        for (i, from) in names.iter().enumerate() {
            for step in 1..=4 {
                graph.add_relationship(EntityRelationship {
                    from_entity: from.clone(),
                    to_entity: names[(i + step) % names.len()].clone(),
                    relation_type: RelationType::RelatedTo,
                    context: String::new(),
                });
            }
        }
        let query = format!("zebracorn rollout {}", names.join(" "));
        let reserve = context_assembly::entity_summary_tokens(&query, &graph);
        assert!((1000..2000).contains(&reserve), "{reserve}");

        let session = memory.get_session(sid).await.unwrap().load_full();
        let mut updates = Vec::new();
        session_updates(&session, &mut updates);
        let ctx = assemble(
            &memory,
            &query,
            &graph,
            &updates,
            &[sid],
            &[],
            2000,
            Corpus::All,
        )
        .await;
        let records = ctx
            .items
            .iter()
            .filter(|i| i.source != ContextSource::Conversation)
            .count();
        assert!(records >= 1, "{ctx:?}");
        assert!(ctx.total_tokens <= 2000, "{}", ctx.total_tokens);
    }
}
