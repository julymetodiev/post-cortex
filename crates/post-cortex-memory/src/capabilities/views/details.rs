// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! [`DetailViews`]: the views the app shows (`pcx.views.v1`). A call
//! resolves its scope once and indexes it once off the async runtime; every
//! view of the call reads that index. Storage-backed numbers (messages, the
//! embedding queue) are read for the scope's sessions a few at a time
//! (`streams`). Limits default and are bounded here, not in the transports.

use std::sync::Arc;
use std::time::Instant;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::capabilities::messages::Messages;
use crate::capabilities::scope::{self, Target};
use crate::capabilities::search::{Search, SearchScope};
use crate::capabilities::sessions::Sessions;
use crate::capabilities::views::streams;
use crate::capabilities::views::watch::{self, ActivityStream, WatchRequest};
use crate::capabilities::views::{Views, ViewsError};
use crate::content_vectorizer::{RerankSkipped, SearchOptions};
use crate::graph_views::detail::{
    EntityDetail, FileDetail, Files, GraphDetail, Overview, PageCursor, RadarDetail, RecordDetail,
    RecordFilter, RecordPage, SearchHitDetail, Timeline, TopicsDetail,
};
use crate::graph_views::{DEFAULT_LIMIT, GraphScope, Index, MAX_LIMIT, ViewError};

/// Hits a detail search returns when none are asked for.
pub const DEFAULT_HITS: usize = 10;
/// Most hits a detail search returns.
pub const MAX_HITS: usize = 100;

/// Newest messages of a session read for a speaker's conversations (did it
/// speak there, how does the conversation stand): a bounded page keeps the
/// listing cheap. The authors of a stream come from its summary instead.
pub const AUTHOR_SCAN: usize = 500;

/// `raw`, or `default` when unset, within 1..=`max`.
fn bounded(raw: Option<usize>, default: usize, max: usize) -> usize {
    raw.unwrap_or(default).clamp(1, max)
}

fn limit(raw: Option<usize>) -> usize {
    bounded(raw, DEFAULT_LIMIT, MAX_LIMIT)
}

/// What an entity page shows; `None` takes the default.
#[derive(Clone, Debug, Default)]
pub struct EntityRequest {
    /// Name or alias.
    pub name: String,
    /// Most records and decisions (default 100, at most 5000).
    pub limit: Option<usize>,
    /// Hops of impact analysis (default 3, at most 10).
    pub impact_depth: Option<usize>,
    /// Hops of the causal chain (default 3, at most 10).
    pub lineage_depth: Option<usize>,
}

/// Search results with the time the search took.
#[derive(Clone, Debug)]
pub struct SearchDetail {
    /// Best first.
    pub hits: Vec<SearchHitDetail>,
    /// Milliseconds the search took (shaping not included).
    pub took_ms: u64,
    /// Neighbours per side the search returns.
    pub neighbour_radius: usize,
    /// Whether a reranker ordered the hits (a `local` or `accurate` search
    /// is not reranked where no reranker for it is configured).
    pub reranked: bool,
    /// Why a search that asked for a reranker got none.
    pub rerank_skipped: Option<RerankSkipped>,
}

/// The detail views of the Views capability (see the module docs).
#[derive(Clone)]
pub struct DetailViews {
    views: Views,
    sessions: Sessions,
    messages: Messages,
    search: Search,
}

/// Index `scope` off the async runtime and run `view` on the index.
async fn on_index<T: Send + 'static>(
    scope: GraphScope,
    view: impl FnOnce(&Index<'_>) -> T + Send + 'static,
) -> Result<T, ViewsError> {
    tokio::task::spawn_blocking(move || view(&Index::of(&scope)))
        .await
        .map_err(|_| ViewsError::RenderFailed)
}

impl DetailViews {
    /// The detail views over the scopes of `views`, reading streams through
    /// `messages`, embedding queues through `sessions`, searching with
    /// `search`.
    pub fn new(views: Views, sessions: Sessions, messages: Messages, search: Search) -> Self {
        Self {
            views,
            sessions,
            messages,
            search,
        }
    }

    async fn scope(&self, target: Target) -> Result<GraphScope, ViewsError> {
        Ok(scope::resolve(&self.views.rt, target).await?.scope)
    }

    async fn indexed<T: Send + 'static>(
        &self,
        target: Target,
        view: impl FnOnce(&Index<'_>) -> T + Send + 'static,
    ) -> Result<T, ViewsError> {
        on_index(self.scope(target).await?, view).await
    }

    /// The `limit` most important entities of `target` (default 100, at
    /// most 5000) and the relations between them; with `at`, as the graph
    /// stood then.
    ///
    /// # Errors
    /// The scope could not be resolved.
    pub async fn graph(
        &self,
        target: Target,
        limit: Option<usize>,
        at: Option<DateTime<Utc>>,
    ) -> Result<GraphDetail, ViewsError> {
        let max = self::limit(limit);
        self.indexed(target, move |index| index.graph_detail(max, at))
            .await
    }

    /// Records written and ended, relations added and ended, in a window
    /// (inclusive, open ends unset); the latest `limit` (default 100).
    ///
    /// # Errors
    /// The scope could not be resolved.
    pub async fn timeline(
        &self,
        target: Target,
        from: Option<DateTime<Utc>>,
        to: Option<DateTime<Utc>>,
        limit: Option<usize>,
    ) -> Result<Timeline, ViewsError> {
        let max = self::limit(limit);
        self.indexed(target, move |index| index.timeline(from, to, max))
            .await
    }

    /// A page of the records matching `filter`, newest first, after
    /// `cursor` (default 100 per page).
    ///
    /// # Errors
    /// The scope could not be resolved.
    pub async fn records(
        &self,
        target: Target,
        filter: RecordFilter,
        limit: Option<usize>,
        cursor: Option<PageCursor<Uuid>>,
    ) -> Result<RecordPage, ViewsError> {
        let max = self::limit(limit);
        self.indexed(target, move |index| index.record_page(&filter, max, cursor))
            .await
    }

    /// One record of `target` in full.
    ///
    /// # Errors
    /// `RecordNotFound`, or the scope could not be resolved.
    pub async fn record(&self, target: Target, id: Uuid) -> Result<RecordDetail, ViewsError> {
        self.indexed(target, move |index| index.record_detail(id))
            .await?
            .ok_or(ViewsError::RecordNotFound(id))
    }

    /// The records of `target` with `ids`, in that order, in full; one that
    /// is not there is left out.
    ///
    /// # Errors
    /// The scope could not be resolved.
    pub async fn records_of(
        &self,
        target: Target,
        ids: Vec<Uuid>,
    ) -> Result<Vec<RecordDetail>, ViewsError> {
        self.indexed(target, move |index| {
            ids.iter()
                .filter_map(|id| index.record_detail(*id))
                .collect()
        })
        .await
    }

    /// The overview of `target`: counts, activity, the `top` most important
    /// entities (default 30) and the `recent` newest decisions and records
    /// (default 10), the radar's counts, messages and the embedding queue.
    ///
    /// # Errors
    /// The scope, a message stream or an embedding state could not be read.
    pub async fn overview(
        &self,
        target: Target,
        top: Option<usize>,
        recent: Option<usize>,
    ) -> Result<Overview, ViewsError> {
        let resolved = scope::resolve(&self.views.rt, target).await?;
        let dismissals = self.views.dismissals_of(&resolved.workspaces).await?;
        let scope = resolved.scope;
        let ids: Vec<Uuid> = scope.sessions.iter().map(|s| s.id()).collect();
        let stats = streams::stats(&self.messages, ids.clone()).await?;
        let embedding_queue = streams::pending(&self.sessions, ids).await?;
        let top = bounded(top, 30, MAX_LIMIT);
        let recent = bounded(recent, 10, MAX_LIMIT);
        let now = Utc::now();
        let mut overview = on_index(scope, move |index| {
            index.overview(now, top, recent, &dismissals)
        })
        .await?;
        overview.messages = stats.iter().map(|s| s.count).sum();
        overview.embedding_queue = embedding_queue;
        overview.last_activity = overview
            .last_activity
            .max(stats.iter().filter_map(|s| s.last_at).max());
        Ok(overview)
    }

    /// An entity's page (see [`EntityRequest`]).
    ///
    /// # Errors
    /// `InvalidArgument` for an empty name, `EntityNotFound`, or the scope
    /// could not be resolved.
    pub async fn entity(
        &self,
        target: Target,
        req: EntityRequest,
    ) -> Result<EntityDetail, ViewsError> {
        let name = req.name.trim().to_string();
        if name.is_empty() {
            return Err(ViewsError::InvalidArgument(
                "name: must not be empty".into(),
            ));
        }
        let max = limit(req.limit);
        let impact = bounded(req.impact_depth, 3, 10);
        let lineage = bounded(req.lineage_depth, 3, 10);
        let wanted = name.clone();
        self.indexed(target, move |index| {
            index.entity_detail(&wanted, max, impact, lineage)
        })
        .await?
        .ok_or(ViewsError::View(ViewError::EntityNotFound(name)))
    }

    /// Topic communities (default 100, at most 5000) with `per_topic`
    /// central entities and key decisions (default 5, at most 50).
    ///
    /// # Errors
    /// The scope could not be resolved.
    pub async fn topics(
        &self,
        target: Target,
        limit: Option<usize>,
        per_topic: Option<usize>,
    ) -> Result<TopicsDetail, ViewsError> {
        let max = self::limit(limit);
        let per_topic = bounded(per_topic, 5, 50);
        self.indexed(target, move |index| index.topics_detail(max, per_topic))
            .await
    }

    /// What needs attention, without the items the scope's workspaces
    /// dismissed unless `include_dismissed` (default 100 per list).
    ///
    /// # Errors
    /// The scope or the dismissals could not be read.
    pub async fn radar(
        &self,
        target: Target,
        limit: Option<usize>,
        include_dismissed: bool,
    ) -> Result<RadarDetail, ViewsError> {
        let resolved = scope::resolve(&self.views.rt, target).await?;
        let dismissals = self.views.dismissals_of(&resolved.workspaces).await?;
        let max = self::limit(limit);
        let now = Utc::now();
        on_index(resolved.scope, move |index| {
            index.radar_detail(now, max, &dismissals, include_dismissed)
        })
        .await
    }

    /// A page of the files the scope's records name (code references and
    /// the files of code changes) whose path contains `text`, after
    /// `cursor`, at most `limit` (default 100).
    ///
    /// # Errors
    /// The scope could not be resolved.
    pub async fn files(
        &self,
        target: Target,
        text: String,
        limit: Option<usize>,
        cursor: Option<PageCursor<String>>,
    ) -> Result<Files, ViewsError> {
        let max = self::limit(limit);
        self.indexed(target, move |index| index.files(&text, max, cursor))
            .await
    }

    /// What the records say about the file `path` (impact `depth` default
    /// 2, at most 10; `limit` records per list, default 20).
    ///
    /// # Errors
    /// `InvalidArgument` for an empty path, or the scope could not be
    /// resolved.
    pub async fn file(
        &self,
        target: Target,
        path: String,
        depth: Option<usize>,
        limit: Option<usize>,
    ) -> Result<FileDetail, ViewsError> {
        let path = path.trim().to_string();
        if path.is_empty() {
            return Err(ViewsError::InvalidArgument(
                "path: must not be empty".into(),
            ));
        }
        let depth = bounded(depth, 2, 10);
        let max = bounded(limit, 20, MAX_LIMIT);
        self.indexed(target, move |index| index.file_detail(&path, depth, max))
            .await
    }

    /// Search `target` for `query` (`options.limit` default
    /// [`DEFAULT_HITS`], at most [`MAX_HITS`]), every hit with its trace and
    /// its neighbours, every record and turn found in full.
    ///
    /// # Errors
    /// The scope could not be resolved, the search failed, or a turn found
    /// could not be read.
    pub async fn search(
        &self,
        target: Target,
        query: &str,
        mut options: SearchOptions,
    ) -> Result<SearchDetail, ViewsError> {
        options.limit = Some(bounded(options.limit, DEFAULT_HITS, MAX_HITS));
        let scope = self.scope(target).await?;
        let scope_of = match target {
            Target::Session(id) => SearchScope::Session(id),
            Target::Workspace(id) => SearchScope::Workspace(id),
            Target::Global => SearchScope::Global,
        };
        let started = Instant::now();
        let found = self.search.find(scope_of, query, options).await?;
        let took_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let (results, reranked, rerank_skipped) =
            (found.hits, found.reranked, found.rerank_skipped);
        let turns = streams::turns(&self.messages, &results).await?;
        let hits = on_index(scope, move |index| index.search_hits(results, &turns)).await?;
        Ok(SearchDetail {
            hits,
            took_ms,
            neighbour_radius: self.search.neighbour_radius(),
            reranked,
            rerank_skipped,
        })
    }

    /// Watch the records of `target` as they are stored: those written
    /// after `request.since` first (none without it: only records stored
    /// from the call on), then each change, every record in full (see
    /// [`WatchRequest`]).
    ///
    /// # Errors
    /// The scope could not be resolved.
    pub async fn watch(
        &self,
        target: Target,
        request: WatchRequest,
    ) -> Result<ActivityStream, ViewsError> {
        // Subscribed before the scope is read, so nothing written in between
        // is missed: records stamped from `since` on come from the read.
        let changes = self.views.rt.changes.subscribe();
        let since = request.since.unwrap_or_else(Utc::now);
        let scope = self.scope(target).await?;
        Ok(watch::watch(
            watch::Sources {
                rt: Arc::clone(&self.views.rt),
                workspaces: self.views.workspaces.clone(),
                messages: self.messages.clone(),
            },
            target,
            request,
            since,
            scope,
            changes,
        ))
    }
}

mod conversations;
#[cfg(test)]
mod tests;
