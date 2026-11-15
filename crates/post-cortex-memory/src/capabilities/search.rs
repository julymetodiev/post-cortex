// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Search: semantic search over one session, several sessions, a
//! workspace or every session, and related content (ARCHITECTURE section
//! 4). Every scope runs one pipeline ([`Search::search`]): the scope is
//! resolved once, records without vectors are embedded first, the
//! vectorizer ranks, and the hits are completed from their sessions.

use std::collections::HashMap;
use std::sync::Arc;

use post_cortex_core::graph::retrieval_graph::RetrievalGraph;
use post_cortex_core::session::active_session::ActiveSession;
use tracing::{debug, warn};
use uuid::Uuid;

use crate::capabilities::search::completion::Completion;
use crate::content_vectorizer::{
    ContentVectorizer, RERANK_POOL, SearchMode, SearchOptions, SemanticSearchResult, VectorScope,
};
use crate::memory_system::MemoryRuntime;

mod completion;

/// Why a search failed.
#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    /// The session of a session search does not exist.
    #[error("Session {0} not found")]
    SessionNotFound(Uuid),
    /// The request is malformed.
    #[error("{0}")]
    InvalidArgument(&'static str),
    /// The workspace of a workspace search does not exist.
    #[error("Workspace {0} not found")]
    WorkspaceNotFound(Uuid),
    /// Embeddings are off in the configuration, so there is nothing to
    /// search with; the same for every scope and transport.
    #[error("Embeddings not enabled or initialized")]
    EmbeddingsDisabled,
    /// The vectorizer could not start or the search failed.
    #[error(transparent)]
    Runtime(#[from] crate::memory_system::RuntimeError),
}

impl From<post_cortex_ports::PortError> for SearchError {
    fn from(e: post_cortex_ports::PortError) -> Self {
        SearchError::Runtime(e.into())
    }
}

/// What a search covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchScope {
    /// Every session and every workspace's documents (nothing when
    /// cross-session search is off).
    Global,
    /// One session; reranked searches also rank by its retrieval graph.
    Session(Uuid),
    /// These sessions (none: no results).
    Sessions(Vec<Uuid>),
    /// The sessions and the documents of this workspace.
    Workspace(Uuid),
}

/// What a search found: its hits and whether a reranker ordered them.
#[derive(Debug, Clone)]
pub struct Found {
    /// The hits, best first, completed from their sessions.
    pub hits: Vec<SemanticSearchResult>,
    /// A reranker ordered them: `local` or `accurate` where one is
    /// configured, the remote one only where every workspace of the scope
    /// allows it.
    pub reranked: bool,
}

/// The Search capability service.
#[derive(Clone)]
pub struct Search {
    rt: Arc<MemoryRuntime>,
    completion: Completion,
}

impl Search {
    pub(crate) fn new(rt: Arc<MemoryRuntime>) -> Self {
        Self {
            completion: Completion::new(Arc::clone(&rt)),
            rt,
        }
    }

    /// Turns on each side of a matched message that search returns with it.
    #[must_use]
    pub fn neighbour_radius(&self) -> usize {
        self.rt.config.neighbour_radius
    }

    /// Whether the configuration turns embeddings (and so search) on.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.rt.config.enable_embeddings
    }

    /// The sessions (and, for a workspace, the workspace itself: the source
    /// of its documents' entries) a search of `scope` covers; none: every
    /// session.
    fn sources(&self, scope: &SearchScope) -> Result<Option<Vec<Uuid>>, SearchError> {
        Ok(match scope {
            SearchScope::Global => None,
            SearchScope::Session(id) => Some(vec![*id]),
            SearchScope::Sessions(ids) => Some(ids.clone()),
            SearchScope::Workspace(id) => Some(
                self.rt
                    .workspace_manager
                    .get_workspace(id)
                    .ok_or(SearchError::WorkspaceNotFound(*id))?
                    .get_all_sessions()
                    .into_iter()
                    .map(|(sid, _)| sid)
                    .chain(std::iter::once(*id))
                    .collect(),
            ),
        })
    }

    /// Whether a search in `mode` over `sources` (none: every session and
    /// every workspace's documents) may use the remote reranker, which
    /// receives the query and the candidates' text: only when it would
    /// (accurate, one configured) and every source allows it. A workspace
    /// allows it when a person turned it on; a session when it is in a
    /// workspace and every workspace it is in allows it.
    ///
    /// # Errors
    /// `Failed` when the sessions of a global search cannot be listed.
    async fn remote_allowed(
        &self,
        vectorizer: &ContentVectorizer,
        mode: Option<SearchMode>,
        sources: Option<&[Uuid]>,
    ) -> Result<bool, SearchError> {
        if !vectorizer.would_rerank_remotely(mode) {
            return Ok(false);
        }
        let workspaces = self.rt.workspace_manager.list_workspaces();
        let every;
        let sources = match sources {
            Some(ids) => ids,
            None => {
                every = self
                    .rt
                    .list_sessions()
                    .await?
                    .into_iter()
                    .chain(workspaces.iter().map(|ws| ws.id))
                    .collect::<Vec<_>>();
                &every
            }
        };
        let allows = |id: &Uuid| match workspaces.iter().find(|ws| ws.id == *id) {
            Some(workspace) => workspace.remote_rerank(),
            None => {
                let mut of = workspaces
                    .iter()
                    .filter(|ws| ws.get_session_role(id).is_some())
                    .peekable();
                of.peek().is_some() && of.all(|ws| ws.remote_rerank())
            }
        };
        Ok(!sources.is_empty() && sources.iter().all(allows))
    }
}

/// Results a semantic search returns when the caller sets no limit.
const DEFAULT_SEARCH_LIMIT: usize = 20;

/// Related-content results when the caller sets no limit.
const DEFAULT_RELATED_LIMIT: usize = 10;

/// How many more candidates a search filtered by interaction type ranks, so
/// the filter still fills the limit.
const TYPE_FILTER_OVERFETCH: usize = 4;

/// Whether `hit` is a record of its session whose interaction type is one
/// of `types`.
fn is_record_of(
    hit: &SemanticSearchResult,
    loaded: &HashMap<Uuid, Arc<ActiveSession>>,
    types: &[post_cortex_core::core::context_update::UpdateType],
) -> bool {
    let Ok(record_id) = Uuid::parse_str(&hit.content_id) else {
        return false;
    };
    let Some(session) = hit.source.session().and_then(|sid| loaded.get(&sid)) else {
        return false;
    };
    session
        .records()
        .iter()
        .any(|r| r.id == record_id && types.contains(&r.update_type))
}

impl Search {
    /// Semantic search over `scope`. `options.limit` defaults to 20.
    ///
    /// A session search embeds the session's records that have no vector
    /// first and, when reranked, adds the session's retrieval graph to the
    /// rankings; the other scopes embed every stored record without a
    /// vector once per instance. Hits come back completed from their
    /// sessions: full text, validity, and the neighbouring records and
    /// turns. An accurate search uses the remote reranker only when every
    /// workspace of its scope allows it (see `remote_allowed`); otherwise
    /// the local one, if any.
    ///
    /// # Errors
    /// `EmbeddingsDisabled` when the configuration turns embeddings off (in
    /// every scope), `SessionNotFound` / `WorkspaceNotFound` for an unknown
    /// scope, else `Failed` (the vectorizer could not start or the search
    /// failed).
    pub async fn search(
        &self,
        scope: SearchScope,
        query: &str,
        options: SearchOptions,
    ) -> Result<Vec<SemanticSearchResult>, SearchError> {
        Ok(self.find(scope, query, options).await?.hits)
    }

    /// [`Search::search`], and whether a reranker ordered the hits: what a
    /// transport that reports it answers.
    ///
    /// # Errors
    /// As [`Search::search`].
    pub async fn find(
        &self,
        scope: SearchScope,
        query: &str,
        mut options: SearchOptions,
    ) -> Result<Found, SearchError> {
        if query.trim().is_empty() {
            return Err(SearchError::InvalidArgument("query cannot be empty"));
        }
        self.require_embeddings()?;
        let rt = &self.rt;
        let vectorizer = rt
            .ensure_vectorizer_initialized()
            .await
            .map_err(SearchError::from)?;
        // A session search (not a workspace or set that has one session)
        // prepares its session and ranks by its graph.
        let one = match scope {
            SearchScope::Session(id) => Some(id),
            _ => None,
        };
        let sessions = self.sources(&scope)?;
        // Only the sources decide; a caller's value is replaced.
        options.remote_rerank = self
            .remote_allowed(&vectorizer, options.mode, sessions.as_deref())
            .await?;
        let reranked = vectorizer.reranks(options.mode, options.remote_rerank);

        // The session of a one-session search, loaded for the graph and the
        // completion.
        let mut loaded: HashMap<Uuid, Arc<ActiveSession>> = HashMap::new();
        if let Some(session_id) = one {
            let session = self.prepare_session(&vectorizer, session_id).await?;
            if reranked {
                options.graph_candidates =
                    Some(self.graph_candidates(&vectorizer, &session, query).await?);
            }
            loaded.insert(session_id, session);
        }

        let limit = *options.limit.get_or_insert(DEFAULT_SEARCH_LIMIT);
        let update_types = std::mem::take(&mut options.update_types);
        let passage = options.passage.take();
        let neighbours = !options.without_neighbours;
        let ranked = if update_types.is_empty() {
            limit
        } else {
            limit * TYPE_FILTER_OVERFETCH
        };
        let vector_scope = match &sessions {
            None => VectorScope::All,
            Some(ids) => VectorScope::Sessions(ids),
        };
        let mut results = vectorizer
            .search(query, ranked, vector_scope, options)
            .await
            .map_err(SearchError::from)?
            .to_vec();
        if !update_types.is_empty() {
            self.completion.load_sessions(&results, &mut loaded).await;
            results.retain(|hit| is_record_of(hit, &loaded, &update_types));
            results.truncate(limit);
        }
        let mut completed = self.completion.complete(results, loaded, neighbours).await;
        if let Some(max_chars) = passage {
            for hit in &mut completed {
                if let Some(shown) =
                    post_cortex_core::text::passage(&hit.text_content, query, max_chars)
                {
                    hit.full_chars = Some(hit.text_content.chars().count());
                    hit.text_content = shown;
                }
                // A long title (often the text's first sentence) shown as
                // its passage too, at a quarter of the text's.
                if let Some(title) = &hit.title
                    && let Some(shown) =
                        post_cortex_core::text::passage(title, query, max_chars / 4)
                {
                    hit.title = Some(shown);
                }
            }
        }
        Ok(Found {
            hits: completed,
            reranked,
        })
    }

    /// Content related to `topic` in other sessions than `session_id`.
    /// `limit` defaults to 10.
    ///
    /// # Errors
    /// `EmbeddingsDisabled`, `SessionNotFound`, or `Failed` (the vectorizer
    /// could not start or the search failed).
    pub async fn related(
        &self,
        session_id: Uuid,
        topic: &str,
        limit: Option<usize>,
    ) -> Result<Vec<SemanticSearchResult>, SearchError> {
        if topic.trim().is_empty() {
            return Err(SearchError::InvalidArgument("topic cannot be empty"));
        }
        self.require_embeddings()?;
        let vectorizer = self
            .rt
            .ensure_vectorizer_initialized()
            .await
            .map_err(SearchError::from)?;
        let session = self.prepare_session(&vectorizer, session_id).await?;
        vectorizer
            .find_related_content(&session, topic, limit.unwrap_or(DEFAULT_RELATED_LIMIT))
            .await
            .map_err(SearchError::from)
    }

    /// `EmbeddingsDisabled` unless the configuration turns embeddings on.
    fn require_embeddings(&self) -> Result<(), SearchError> {
        if self.is_enabled() {
            Ok(())
        } else {
            Err(SearchError::EmbeddingsDisabled)
        }
    }

    /// Load an existing session and embed its records that have no vector
    /// yet (a failure is logged: the search runs on the vectors there are).
    async fn prepare_session(
        &self,
        vectorizer: &ContentVectorizer,
        session_id: Uuid,
    ) -> Result<Arc<ActiveSession>, SearchError> {
        let rt = &self.rt;
        let session = rt
            .session_manager
            .get_session(session_id)
            .await
            .map_err(SearchError::from)?
            .ok_or(SearchError::SessionNotFound(session_id))?
            .load_full();
        if !vectorizer.needs_vectorization(&session) {
            return Ok(session);
        }
        debug!("Session {session_id} not vectorized, auto-vectorizing before search");
        if let Err(e) = rt.vectorize_session_updates(session_id).await {
            warn!("Auto-vectorization failed for session {session_id}: {e}");
            return Ok(session);
        }
        // The snapshot that holds the new vectorized ids.
        Ok(rt
            .session_manager
            .cached(session_id)
            .map_or(session, |handle| handle.load_full()))
    }

    /// Content ids the session's retrieval graph ranks for `query`, best
    /// first. The graph joins the session's records through the terms and
    /// agent entities stored with their vectors and the entity relations; it
    /// is cached per session and rebuilt when the session's vector or
    /// relation count changes.
    async fn graph_candidates(
        &self,
        vectorizer: &ContentVectorizer,
        session: &ActiveSession,
        query: &str,
    ) -> Result<Arc<[String]>, SearchError> {
        let session_id = session.id();
        let vectors = vectorizer
            .count_session_embeddings(session_id)
            .await
            .map_err(SearchError::from)?;
        let relations = session.entity_graph().edge_count();
        let graphs = &self.rt.session_manager.retrieval_graphs;
        let cached = graphs
            .get(&session_id)
            .filter(|entry| entry.0 == vectors && entry.1 == relations)
            .map(|entry| Arc::clone(&entry.2));
        let graph = match cached {
            Some(graph) => graph,
            None => {
                let records = vectorizer
                    .graph_records(session_id)
                    .await
                    .map_err(SearchError::from)?;
                let graph = Arc::new(RetrievalGraph::build(
                    records,
                    &session.entity_graph().get_all_relationships(),
                ));
                graphs.insert(session_id, (vectors, relations, Arc::clone(&graph)));
                graph
            }
        };
        Ok(graph.rank(query, RERANK_POOL).into())
    }
}

#[cfg(test)]
mod retrieval_pin_tests;
#[cfg(test)]
mod tests;
