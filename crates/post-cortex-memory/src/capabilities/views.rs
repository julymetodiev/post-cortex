// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Views: graph views over a session or workspace for agents (entity,
//! impact, lineage, topics, radar, file), the detail views the app shows
//! ([`DetailViews`], `pcx.views.v1`), the structured summary, key decisions
//! and insights, and the radar dismissals of a workspace (ARCHITECTURE
//! section 4).

use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::Utc;
use post_cortex_core::core::dismissal::{MAX_DISMISSAL_KEY_BYTES, RadarDismissal, RadarItemKind};
use post_cortex_core::summary::{DecisionSummary, SummaryGenerator};
use uuid::Uuid;

use crate::capabilities::messages::MessageError;
use crate::capabilities::scope::{self, ScopeError, Target};
use crate::capabilities::search::SearchError;
use crate::capabilities::sessions::SessionError;
use crate::capabilities::workspaces::Workspaces;
use crate::graph_views::{RenderedView, ViewError, ViewRequest};
use crate::memory_system::MemoryRuntime;

mod details;
mod graph;
mod query;
mod streams;
mod summary;
mod watch;

pub use details::{AUTHOR_SCAN, DEFAULT_HITS, DetailViews, EntityRequest, MAX_HITS, SearchDetail};
pub use watch::{ActivityEvent, ActivityKind, ActivityStream, WatchRequest};

pub use graph::{
    DEFAULT_IMPORTANCE_LIMIT, DEFAULT_NETWORK_ENTITIES, DEFAULT_NETWORK_RELATIONSHIPS,
    EntityImportance, EntityNetworkView, ImportancePage, NetworkEntity, NetworkRelationship,
    SessionStatistics,
};
pub use query::{ContextQuery, ContextResponse};
pub use summary::{
    Compaction, DEFAULT_INSIGHTS_LIMIT, MAX_SUMMARY_TOKENS, StructuredSummary, SummaryRequest,
    SummarySection,
};

/// Why a view failed.
#[derive(Debug, thiserror::Error)]
pub enum ViewsError {
    /// The scope could not be resolved.
    #[error(transparent)]
    Scope(#[from] ScopeError),
    /// The view's entity is not in the scope.
    #[error(transparent)]
    View(#[from] ViewError),
    /// The request is malformed.
    #[error("{0}")]
    InvalidArgument(String),
    /// No such workspace (dismissals).
    #[error("{0}")]
    WorkspaceNotFound(String),
    /// Storage failed.
    #[error("{0}")]
    Storage(#[from] crate::memory_system::RuntimeError),
    /// No such record in the scope.
    #[error("record {0} not found in the scope")]
    RecordNotFound(Uuid),
    /// Reading a message stream failed.
    #[error(transparent)]
    Messages(#[from] MessageError),
    /// Reading a session's embedding state failed.
    #[error(transparent)]
    Sessions(#[from] SessionError),
    /// The search failed.
    #[error(transparent)]
    Search(#[from] SearchError),
    /// The rendering task panicked or was cancelled.
    #[error("rendering the view did not finish")]
    RenderFailed,
}

/// The Views capability service.
#[derive(Clone)]
pub struct Views {
    rt: Arc<MemoryRuntime>,
    workspaces: Workspaces,
}

impl Views {
    pub(crate) fn new(rt: Arc<MemoryRuntime>, workspaces: Workspaces) -> Self {
        Self { rt, workspaces }
    }

    /// Render `view` over `target`; a radar view leaves out the items the
    /// scope's workspaces dismissed. Rendering runs off the async runtime.
    ///
    /// # Errors
    /// The scope could not be resolved, the entity is not in it, or the
    /// dismissals could not be read.
    pub async fn view(
        &self,
        target: Target,
        view: ViewRequest,
    ) -> Result<RenderedView, ViewsError> {
        let resolved = scope::resolve(&self.rt, target).await?;
        let dismissals = match &view {
            ViewRequest::Radar { .. } => self.dismissals_of(&resolved.workspaces).await?,
            _ => Vec::new(),
        };
        self.render(resolved, view, dismissals).await
    }

    async fn render(
        &self,
        resolved: scope::Resolved,
        view: ViewRequest,
        dismissals: Vec<RadarDismissal>,
    ) -> Result<RenderedView, ViewsError> {
        let scope = resolved.scope;
        let now = Utc::now();
        tokio::task::spawn_blocking(move || scope.render_with(&view, &dismissals, now))
            .await
            .map_err(|_| ViewsError::RenderFailed)?
            .map_err(ViewsError::View)
    }

    /// The structured summary of a session (see [`SummaryRequest`]),
    /// compacted when it would be too large.
    ///
    /// # Errors
    /// The session does not exist or cannot be loaded.
    pub async fn summary(&self, req: SummaryRequest) -> Result<StructuredSummary, ViewsError> {
        let session = scope::load(&self.rt, req.session_id).await?;
        Ok(summary::build(&session, req, Utc::now()))
    }

    /// The session's decisions, oldest first.
    ///
    /// # Errors
    /// The session does not exist or cannot be loaded.
    pub async fn key_decisions(
        &self,
        session_id: Uuid,
    ) -> Result<Vec<DecisionSummary>, ViewsError> {
        let session = scope::load(&self.rt, session_id).await?;
        Ok(SummaryGenerator::extract_decision_timeline(&session))
    }

    /// The session's key insights, at most `limit`
    /// ([`DEFAULT_INSIGHTS_LIMIT`] when None).
    ///
    /// # Errors
    /// The session does not exist or cannot be loaded.
    pub async fn key_insights(
        &self,
        session_id: Uuid,
        limit: Option<usize>,
    ) -> Result<Vec<String>, ViewsError> {
        let session = scope::load(&self.rt, session_id).await?;
        Ok(SummaryGenerator::extract_key_insights(
            &session,
            limit.unwrap_or(DEFAULT_INSIGHTS_LIMIT),
        ))
    }

    /// The session's entities ranked by importance: at most `limit`
    /// ([`DEFAULT_IMPORTANCE_LIMIT`]), those at or above `min_importance`.
    ///
    /// # Errors
    /// The session does not exist or cannot be loaded.
    pub async fn entity_importance(
        &self,
        session_id: Uuid,
        limit: Option<usize>,
        min_importance: Option<f32>,
    ) -> Result<EntityImportance, ViewsError> {
        let session = scope::load(&self.rt, session_id).await?;
        Ok(graph::importance(
            session.entity_graph(),
            limit,
            min_importance,
        ))
    }

    /// The network two hops around `center` (else the most important
    /// entity), closest first, bounded by `max_entities`
    /// ([`DEFAULT_NETWORK_ENTITIES`]) and `max_relationships`
    /// ([`DEFAULT_NETWORK_RELATIONSHIPS`]).
    ///
    /// # Errors
    /// The session does not exist or cannot be loaded.
    pub async fn entity_network(
        &self,
        session_id: Uuid,
        center: Option<&str>,
        max_entities: Option<usize>,
        max_relationships: Option<usize>,
    ) -> Result<EntityNetworkView, ViewsError> {
        let session = scope::load(&self.rt, session_id).await?;
        let graph = session.entity_graph();
        Ok(graph::network(
            session_id,
            graph,
            center,
            max_entities,
            max_relationships,
        ))
    }

    /// Size and activity of a session.
    ///
    /// # Errors
    /// The session does not exist or cannot be loaded.
    pub async fn statistics(&self, session_id: Uuid) -> Result<SessionStatistics, ViewsError> {
        let session = scope::load(&self.rt, session_id).await?;
        Ok(graph::statistics(session_id, &session))
    }

    /// Answer `query` over the session.
    ///
    /// # Errors
    /// `InvalidArgument` when a text the query needs (an entity name, a
    /// file path, a search text) is empty; the session does not exist or
    /// cannot be loaded.
    pub async fn query(
        &self,
        session_id: Uuid,
        query: ContextQuery,
    ) -> Result<ContextResponse, ViewsError> {
        if let Some(field) = query::missing_field(&query) {
            return Err(ViewsError::InvalidArgument(format!(
                "{field} is required for this query"
            )));
        }
        let session = scope::load(&self.rt, session_id).await?;
        Ok(query::answer(&session, query, chrono::Utc::now()))
    }

    /// Dismiss the radar item of `kind` and `key` in `workspace_id` (the key
    /// is trimmed; see [`post_cortex_core::core::dismissal`] for what it
    /// names). Dismissing it again replaces the dismissal (time and
    /// author). Returns the dismissal stored.
    ///
    /// # Errors
    /// `WorkspaceNotFound`, `InvalidArgument` for a bad key, `Storage`.
    pub async fn dismiss_radar_item(
        &self,
        workspace_id: Uuid,
        kind: RadarItemKind,
        key: &str,
        author: Option<String>,
    ) -> Result<RadarDismissal, ViewsError> {
        self.require_workspace(workspace_id)?;
        let dismissal = RadarDismissal {
            kind,
            key: normalize_key(key)?,
            dismissed_at: Utc::now(),
            author: author
                .map(|a| a.trim().to_string())
                .filter(|a| !a.is_empty()),
        };
        self.rt
            .stores
            .dismissals
            .put_dismissal(workspace_id, &dismissal)
            .await
            .map_err(storage)?;
        Ok(dismissal)
    }

    /// Restore a dismissed radar item; returns whether it was dismissed.
    ///
    /// # Errors
    /// `WorkspaceNotFound`, `InvalidArgument` for a bad key, `Storage`.
    pub async fn restore_radar_item(
        &self,
        workspace_id: Uuid,
        kind: RadarItemKind,
        key: &str,
    ) -> Result<bool, ViewsError> {
        self.require_workspace(workspace_id)?;
        self.rt
            .stores
            .dismissals
            .delete_dismissal(workspace_id, kind, &normalize_key(key)?)
            .await
            .map_err(storage)
    }

    /// The radar dismissals of `workspace_id`, by kind and key.
    ///
    /// # Errors
    /// `WorkspaceNotFound` or `Storage`.
    pub async fn radar_dismissals(
        &self,
        workspace_id: Uuid,
    ) -> Result<Vec<RadarDismissal>, ViewsError> {
        self.require_workspace(workspace_id)?;
        let mut dismissals = self
            .rt
            .stores
            .dismissals
            .list_dismissals(workspace_id)
            .await
            .map_err(storage)?;
        dismissals.sort_by(|a, b| (a.kind, &a.key).cmp(&(b.kind, &b.key)));
        Ok(dismissals)
    }

    /// The radar dismissals a view of `target` applies: those of the
    /// workspace, or of every workspace the session belongs to (one per
    /// item, the latest).
    ///
    /// # Errors
    /// The scope could not be resolved, or storage failed.
    pub async fn dismissals_for(&self, target: Target) -> Result<Vec<RadarDismissal>, ViewsError> {
        let resolved = scope::resolve(&self.rt, target).await?;
        self.dismissals_of(&resolved.workspaces).await
    }

    /// The dismissals of `workspaces`, one per item (the latest).
    async fn dismissals_of(&self, workspaces: &[Uuid]) -> Result<Vec<RadarDismissal>, ViewsError> {
        let mut by_item: BTreeMap<(RadarItemKind, String), RadarDismissal> = BTreeMap::new();
        for &ws in workspaces {
            let stored = self
                .rt
                .stores
                .dismissals
                .list_dismissals(ws)
                .await
                .map_err(storage)?;
            for d in stored {
                let item = (d.kind, d.key.clone());
                if by_item
                    .get(&item)
                    .is_none_or(|kept| kept.dismissed_at < d.dismissed_at)
                {
                    by_item.insert(item, d);
                }
            }
        }
        Ok(by_item.into_values().collect())
    }

    fn require_workspace(&self, workspace_id: Uuid) -> Result<(), ViewsError> {
        self.workspaces
            .get(workspace_id)
            .map(|_| ())
            .map_err(|e| ViewsError::WorkspaceNotFound(e.to_string()))
    }
}

/// A dismissal key as stored and matched: trimmed, 1 to
/// [`MAX_DISMISSAL_KEY_BYTES`] bytes.
fn normalize_key(key: &str) -> Result<String, ViewsError> {
    let key = key.trim();
    if key.is_empty() || key.len() > MAX_DISMISSAL_KEY_BYTES {
        return Err(ViewsError::InvalidArgument(format!(
            "key must be 1 to {MAX_DISMISSAL_KEY_BYTES} bytes"
        )));
    }
    Ok(key.to_string())
}

/// The one mapping of a storage port error to this capability's error.
fn storage(e: post_cortex_ports::PortError) -> ViewsError {
    ViewsError::Storage(e.into())
}

#[cfg(test)]
mod tests;
