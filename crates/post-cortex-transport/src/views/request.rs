// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! `pcx.views.v1` requests as capability arguments. Parsing is strict: an
//! unknown interaction type or enum value, a timestamp out of range, a
//! malformed id or cursor is `InvalidArgument`, never read as a default.
//! Defaults and bounds of counts are the capability's (a 0 count is unset).

use std::str::FromStr;

use chrono::{DateTime, Utc};
use post_cortex_core::core::context_update::UpdateType;
use post_cortex_memory::capabilities::{Target, WatchRequest};
use post_cortex_memory::content_vectorizer::{SearchMode, SearchOptions};
use post_cortex_memory::graph_views::detail::{
    InvalidCursor, PageCursor, RecordFilter, ValidityFilter,
};
use post_cortex_ports::Corpus;
use post_cortex_proto::views as pb;
use tonic::Status;
use uuid::Uuid;

/// `raw` as a UUID named `what`.
pub fn uuid(raw: &str, what: &str) -> Result<Uuid, Status> {
    Uuid::parse_str(raw.trim()).map_err(|_| Status::invalid_argument(format!("{what}: not a UUID")))
}

/// The target of a request's scope.
pub fn target(scope: Option<pb::Scope>) -> Result<Target, Status> {
    match scope.and_then(|s| s.scope) {
        Some(pb::scope::Scope::SessionId(raw)) => Ok(Target::Session(uuid(&raw, "session_id")?)),
        Some(pb::scope::Scope::WorkspaceId(raw)) => {
            Ok(Target::Workspace(uuid(&raw, "workspace_id")?))
        }
        Some(pb::scope::Scope::Global(true)) => Ok(Target::Global),
        Some(pb::scope::Scope::Global(false)) | None => Err(Status::invalid_argument(
            "scope: a session_id, workspace_id or global: true is required",
        )),
    }
}

/// A timestamp, or `InvalidArgument` when it is not a time.
pub fn time(t: Option<prost_types::Timestamp>) -> Result<Option<DateTime<Utc>>, Status> {
    t.map(|t| {
        u32::try_from(t.nanos)
            .ok()
            .and_then(|nanos| DateTime::from_timestamp(t.seconds, nanos))
            .ok_or_else(|| Status::invalid_argument("timestamp out of range"))
    })
    .transpose()
}

/// A count: 0 is unset (the capability's default).
pub fn count(raw: u32) -> Option<usize> {
    (raw != 0).then_some(raw as usize)
}

/// An optional text: blank is unset.
fn text(raw: Option<String>) -> Option<String> {
    raw.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// The page cursor `raw` (a `next_cursor`); none when empty.
pub fn cursor<K: FromStr>(raw: &str) -> Result<Option<PageCursor<K>>, Status> {
    match raw.trim() {
        "" => Ok(None),
        raw => raw
            .parse()
            .map(Some)
            .map_err(|e: InvalidCursor| Status::invalid_argument(e.to_string())),
    }
}

/// The browser filter and cursor of `req`.
pub fn record_filter(
    req: &pb::ListRecordsRequest,
) -> Result<(RecordFilter, Option<PageCursor<Uuid>>), Status> {
    let types = req
        .interaction_types
        .iter()
        .map(|t| {
            t.parse::<UpdateType>()
                .map_err(|e| Status::invalid_argument(format!("interaction_types: {e}")))
        })
        .collect::<Result<_, _>>()?;
    let validity = match pb::ValidityFilter::try_from(req.validity) {
        Ok(pb::ValidityFilter::ValidityAny) => ValidityFilter::Any,
        Ok(pb::ValidityFilter::ValidityCurrent) => ValidityFilter::Current,
        Ok(pb::ValidityFilter::ValidityEnded) => ValidityFilter::Ended,
        Err(_) => return Err(Status::invalid_argument("validity: unknown value")),
    };
    let cursor = cursor(&req.cursor)?;
    let filter = RecordFilter {
        types,
        author: text(req.author.clone()),
        entity: text(req.entity.clone()),
        text: req.text.clone(),
        validity,
        from: time(req.from)?,
        to: time(req.to)?,
        session: text(req.session_id.clone())
            .map(|raw| uuid(&raw, "session_id"))
            .transpose()?,
    };
    Ok((filter, cursor))
}

/// The search options of `req`; its date range is open where unset.
pub fn search_options(req: &pb::SearchRequest) -> Result<SearchOptions, Status> {
    let mode = match pb::SearchMode::try_from(req.mode) {
        Ok(pb::SearchMode::Unspecified) => None,
        Ok(pb::SearchMode::Fast) => Some(SearchMode::Fast),
        Ok(pb::SearchMode::Local) => Some(SearchMode::Local),
        Ok(pb::SearchMode::Accurate) => Some(SearchMode::Accurate),
        Err(_) => return Err(Status::invalid_argument("mode: unknown value")),
    };
    let corpus = match pb::SearchSources::try_from(req.sources) {
        Ok(pb::SearchSources::Unspecified | pb::SearchSources::Records) => Corpus::Records,
        Ok(pb::SearchSources::Messages) => Corpus::Messages,
        Ok(pb::SearchSources::All) => Corpus::All,
        Err(_) => return Err(Status::invalid_argument("sources: unknown value")),
    };
    let date_range = match (time(req.from)?, time(req.to)?) {
        (None, None) => None,
        (from, to) => Some((
            from.unwrap_or(DateTime::<Utc>::MIN_UTC),
            to.unwrap_or(DateTime::<Utc>::MAX_UTC),
        )),
    };
    Ok(SearchOptions {
        limit: count(req.limit),
        date_range,
        recency_bias: req.recency_bias,
        mode,
        corpus,
        author: text(req.author.clone()),
        prefer_author: text(req.prefer_author.clone()),
        trace: true,
        rerank_withheld: post_cortex_memory::tools::wire::rerank_withheld(
            req.rerank_withheld.as_deref(),
        )
        .map_err(|f| Status::invalid_argument(f.message))?,
        ..SearchOptions::default()
    })
}

/// The target and request of a watch.
pub fn watch(req: pb::WatchActivityRequest) -> Result<(Target, WatchRequest), Status> {
    Ok((
        target(req.scope)?,
        WatchRequest {
            since: time(req.since)?,
            first_only: req.first_only,
            changes: req.changes,
            messages: req.messages,
        },
    ))
}
