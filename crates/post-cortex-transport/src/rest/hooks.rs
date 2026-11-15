// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! REST routes the client hooks call: hook events (to the module host),
//! health, project resolution, message capture and the file memory view.

use crate::rest::ApiState;
use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
};
use pcx_module_api::{EventEnvelope, Outcome, Scope, event_kind};
use post_cortex_core::services::{
    AppendMessagesRequest, AppendMessagesResponse, ResolveProjectRequest, ResolveProjectResponse,
};
use post_cortex_memory::capabilities::Target;
use post_cortex_memory::tools::CapturedMessage;
use serde::Deserialize;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// Longest a hook event may take when its scope sets no deadline.
const EVENT_DEADLINE: Duration = Duration::from_secs(30);

/// Body of `POST /api/projects/resolve`.
#[derive(Deserialize)]
pub struct ResolveProjectBody {
    key: String,
    name: String,
    #[serde(default)]
    client: Option<String>,
    #[serde(default)]
    workspace: Option<String>,
}

/// Body of `POST /api/messages`: captured turns from a client hook.
#[derive(Deserialize)]
pub struct AppendMessagesBody {
    session_id: Uuid,
    messages: Vec<CapturedMessage>,
}

/// `POST /api/events`: a hook event (the `pcx.module.v1` `EventEnvelope`
/// as JSON: its scope and the client's hook JSON) dispatched to every
/// module subscribed to it; the answer is their merged outcome, a Claude
/// Code hook answer the hook prints as it is. A module that fails is
/// logged, and the outcome is what its policy makes of it.
pub async fn api_events(
    State(state): State<Arc<ApiState>>,
    headers: axum::http::HeaderMap,
    Json(envelope): Json<EventEnvelope>,
) -> Result<Json<Outcome>, (StatusCode, String)> {
    let bad = |message: String| (StatusCode::BAD_REQUEST, message);
    let event = envelope
        .event
        .ok_or_else(|| bad("the envelope has no event".to_string()))?;
    let kind = event_kind(&event.hook_event_name)
        .ok_or_else(|| bad(format!("no module answers {}", event.hook_event_name)))?;
    let scope = envelope
        .scope
        .ok_or_else(|| bad("the envelope has no scope".to_string()))?;
    // The request's own end (a dropped connection) cancels the handler.
    let mut scope = Scope::from_wire(scope, CancellationToken::new(), EVENT_DEADLINE);
    // The person is the gateway's, whatever the envelope names.
    scope.person = crate::person::of_headers(&headers)?.map(|id| id.to_string());
    let dispatched = state.host.dispatch(&scope, kind, &event).await;
    for failure in &dispatched.failures {
        tracing::warn!(
            module = failure.module,
            event = event.hook_event_name,
            blocked = failure.blocked,
            error = %failure.error,
            "a module failed a hook event"
        );
    }
    Ok(Json(dispatched.outcome))
}

/// `GET /health`: the payload of MCP `admin health`, with the `status` and
/// `service` keys `/health` always had.
#[derive(serde::Serialize)]
pub struct HealthBody {
    status: &'static str,
    service: &'static str,
    #[serde(flatten)]
    health: post_cortex_memory::tools::responses::HealthPayload,
    /// Every module of the host (T14).
    modules: Vec<ModuleHealthBody>,
}

/// One module's health in `/health`.
#[derive(serde::Serialize)]
pub struct ModuleHealthBody {
    module: String,
    calls: u64,
    errors: u64,
    timeouts: u64,
    panics: u64,
    cancelled: u64,
    refused: u64,
    breaker_open: bool,
    /// Median latency in milliseconds (absent before the first call).
    p50_ms: Option<u64>,
    /// 99th percentile latency in milliseconds; `u64::MAX` past the
    /// slowest bucket.
    p99_ms: Option<u64>,
    /// What stops it from doing its work, as it reported; empty: nothing.
    problem: String,
}

impl From<pcx_host::ModuleHealth> for ModuleHealthBody {
    fn from(h: pcx_host::ModuleHealth) -> Self {
        // Saturates: the slowest bucket is open-ended.
        let ms = |d: Duration| u64::try_from(d.as_millis()).unwrap_or(u64::MAX);
        Self {
            module: h.module,
            calls: h.calls,
            errors: h.errors,
            timeouts: h.timeouts,
            panics: h.panics,
            cancelled: h.cancelled,
            refused: h.refused,
            breaker_open: h.breaker_open,
            p50_ms: h.p50.map(ms),
            p99_ms: h.p99.map(ms),
            problem: h.problem,
        }
    }
}

/// The health payload of MCP `admin health`, with the `status` and
/// `service` keys `/health` always had, and every module's health.
pub async fn api_health(State(state): State<Arc<ApiState>>) -> Json<HealthBody> {
    let payload = HealthBody {
        status: "ok",
        service: "post-cortex",
        health: post_cortex_memory::tools::responses::HealthPayload::from(
            state.memory.admin.health(),
        ),
        modules: state.host.health().into_iter().map(Into::into).collect(),
    };
    Json(payload)
}

/// The workspace and session of a project key, created on first use
/// (idempotent). The client is the body's, else the `X-PCX-Author` header's.
pub async fn api_resolve_project(
    State(state): State<Arc<ApiState>>,
    headers: axum::http::HeaderMap,
    Json(body): Json<ResolveProjectBody>,
) -> Result<Json<ResolveProjectResponse>, (StatusCode, String)> {
    let request = ResolveProjectRequest {
        key: body.key,
        name: body.name,
        workspace: body.workspace,
        client: body.client.or_else(|| crate::mcp::header_author(&headers)),
    };
    state
        .memory
        .projects
        .resolve(request)
        .await
        .map(Json)
        .map_err(crate::rest::rest_status)
}

/// Append captured turns to a session's message stream. Idempotent on each turn's `turn_id`, so hooks can retry.
pub async fn api_append_messages(
    State(state): State<Arc<ApiState>>,
    Json(body): Json<AppendMessagesBody>,
) -> Result<Json<AppendMessagesResponse>, (StatusCode, String)> {
    let now = chrono::Utc::now();
    let request = AppendMessagesRequest {
        session_id: body.session_id,
        messages: body.messages.into_iter().map(|m| m.into_new(now)).collect(),
    };
    state
        .memory
        .messages
        .append(request)
        .await
        .map(Json)
        .map_err(crate::rest::rest_status)
}

pub type Params = std::collections::HashMap<String, String>;

/// Render a graph view for the scope named in `params`:
/// `scope=session|workspace&id=...`, or `session_id=...` /
/// `workspace_id=...`. The other parameters are the view's, see
/// [`post_cortex_memory::graph_views::ViewRequest::parse`].
pub async fn graph_view(
    state: &ApiState,
    view: &str,
    params: Params,
) -> Result<Json<post_cortex_memory::graph_views::RenderedView>, (StatusCode, String)> {
    use post_cortex_memory::graph_views::ViewRequest;

    let uuid = |key: &str| -> Result<Option<Uuid>, (StatusCode, String)> {
        params
            .get(key)
            .filter(|v| !v.is_empty())
            .map(|v| {
                Uuid::parse_str(v)
                    .map_err(|e| (StatusCode::BAD_REQUEST, format!("{key}: invalid UUID: {e}")))
            })
            .transpose()
    };
    let (session, workspace) = match params.get("scope").map(String::as_str) {
        Some("session") => (uuid("id")?, None),
        Some("workspace") => (None, uuid("id")?),
        Some(other) => {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("scope must be 'session' or 'workspace', got '{other}'"),
            ));
        }
        None => (uuid("session_id")?, uuid("workspace_id")?),
    };
    let request = ViewRequest::parse(view, &params).map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    let target =
        Target::of(session, workspace).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    let view = state
        .memory
        .views
        .view(target, request)
        .await
        .map_err(crate::rest::rest_status)?;
    Ok(Json(view))
}

/// `GET /api/file?path=...`: records, past problems, decisions and
/// dependents of a source file (`depth`, `limit`), for editor hooks.
pub async fn api_file(
    State(state): State<Arc<ApiState>>,
    Query(params): Query<Params>,
) -> Result<Json<post_cortex_memory::graph_views::RenderedView>, (StatusCode, String)> {
    graph_view(&state, "file", params).await
}
