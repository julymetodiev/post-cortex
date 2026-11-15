// Copyright (c) 2026 Julius ML
// MIT License

//! The one resolver of a read scope (a session, a workspace's members, or
//! every session, with their merged graph), used by Assembly and Views.
//!
//! One behaviour for every caller: an unknown workspace or session is
//! `NotFound`; a member session that is gone or cannot be loaded is left
//! out of a workspace or global scope (logged); a session that cannot be
//! loaded is `Storage`.

use futures::{StreamExt, stream};
use std::sync::Arc;

use post_cortex_core::graph::entity_graph::SimpleEntityGraph;
use post_cortex_core::session::active_session::ActiveSession;
use tracing::warn;
use uuid::Uuid;

use crate::graph_views::GraphScope;
use crate::memory_system::MemoryRuntime;
use crate::memory_system::workspace_graphs::MergeKey;

/// How many sessions of a scope are read at once.
const SESSIONS_AT_ONCE: usize = 16;

/// What a read covers: one session, or every session of a workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// One session.
    Session(Uuid),
    /// The member sessions of a workspace.
    Workspace(Uuid),
    /// Every session.
    Global,
}

impl Target {
    /// The target of a request naming a session and/or a workspace: the
    /// workspace when given, else the session.
    ///
    /// # Errors
    /// `Missing` when neither is given.
    pub fn of(session: Option<Uuid>, workspace: Option<Uuid>) -> Result<Self, ScopeError> {
        match (workspace, session) {
            (Some(ws), _) => Ok(Self::Workspace(ws)),
            (None, Some(session)) => Ok(Self::Session(session)),
            (None, None) => Err(ScopeError::Missing),
        }
    }
}

/// Why a scope could not be resolved.
#[derive(Debug, thiserror::Error)]
pub enum ScopeError {
    /// Neither a session nor a workspace was named.
    #[error("either a session id or a workspace id is required")]
    Missing,
    /// The session or workspace does not exist.
    #[error("{0} not found")]
    NotFound(String),
    /// The session could not be loaded.
    #[error("{0}")]
    Storage(#[from] crate::memory_system::RuntimeError),
}

/// A resolved scope.
pub(crate) struct Resolved {
    /// The sessions, with the graph over them.
    pub(crate) scope: GraphScope,
    /// The workspaces whose cards apply: the workspace, or every workspace
    /// the session belongs to.
    pub(crate) workspaces: Vec<Uuid>,
}

impl Resolved {
    /// Ids of the sessions in the scope.
    pub(crate) fn session_ids(&self) -> Vec<Uuid> {
        self.scope.sessions.iter().map(|s| s.id()).collect()
    }
}

/// Resolve `target` (see the module docs).
pub(crate) async fn resolve(rt: &MemoryRuntime, target: Target) -> Result<Resolved, ScopeError> {
    match target {
        Target::Workspace(ws_id) => {
            let ws = rt
                .workspace_manager
                .get_workspace(&ws_id)
                .ok_or_else(|| ScopeError::NotFound(format!("workspace {ws_id}")))?;
            let members = ws.get_all_sessions().into_iter().map(|(id, _)| id);
            let scope = merged(rt, MergeKey::Workspace(ws_id), members).await;
            Ok(Resolved {
                scope,
                workspaces: vec![ws_id],
            })
        }
        Target::Global => {
            let all = rt.list_sessions().await.map_err(ScopeError::Storage)?;
            Ok(Resolved {
                scope: merged(rt, MergeKey::Global, all).await,
                workspaces: Vec::new(),
            })
        }
        Target::Session(session_id) => {
            let session = load(rt, session_id).await?;
            let graph: Arc<SimpleEntityGraph> = Arc::clone(session.entity_graph_shared());
            Ok(Resolved {
                scope: GraphScope {
                    sessions: vec![session],
                    graph,
                },
                workspaces: rt
                    .workspace_manager
                    .list_workspaces()
                    .iter()
                    .filter(|ws| ws.get_session_role(&session_id).is_some())
                    .map(|ws| ws.id)
                    .collect(),
            })
        }
    }
}

/// The sessions `ids` (those gone or unreadable left out, logged) with
/// their graphs merged under `key`.
async fn merged(
    rt: &MemoryRuntime,
    key: MergeKey,
    ids: impl IntoIterator<Item = Uuid>,
) -> GraphScope {
    let mut sessions = Vec::new();
    let mut graphs = Vec::new();
    // Each at once: a workspace of many sessions, read cold, waits for the slowest.
    let read: Vec<_> =
        stream::iter(ids)
            .map(|session_id| async move {
                (session_id, rt.session_manager.get_session(session_id).await)
            })
            .buffered(SESSIONS_AT_ONCE)
            .collect()
            .await;
    for (session_id, session) in read {
        match session {
            Ok(Some(handle)) => {
                let session = handle.load_full();
                graphs.push((session_id, Arc::clone(session.entity_graph_shared())));
                sessions.push(session);
            }
            Ok(None) => {}
            Err(e) => warn!("Session {session_id} left out of the {key:?} scope: {e}"),
        }
    }
    GraphScope {
        sessions,
        graph: rt.workspace_graphs.merged(key, graphs),
    }
}

/// The current snapshot of session `session_id`.
pub(crate) async fn load(
    rt: &MemoryRuntime,
    session_id: Uuid,
) -> Result<Arc<ActiveSession>, ScopeError> {
    rt.session_manager
        .get_session(session_id)
        .await
        .map_err(ScopeError::Storage)?
        .map(|handle| handle.load_full())
        .ok_or_else(|| ScopeError::NotFound(format!("session {session_id}")))
}
