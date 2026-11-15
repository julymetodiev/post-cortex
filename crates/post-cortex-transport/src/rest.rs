// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! The REST API for the `pcx` CLI, hooks and plugins: `/health` and
//! `/api/*` over memory and the module host (ARCHITECTURE section 3:
//! transports are thin mappings). The daemon puts its host policy and
//! token in front of these routes.

use std::sync::Arc;

use axum::Router;
use axum::http::StatusCode;
use axum::routing::{delete, get, post, put};
use pcx_host::Host;
use post_cortex_memory::Memory;
use post_cortex_memory::tools::{Failure, Kind};

use cards::{api_get_cards, api_put_user_card, api_put_workspace_card};
use hooks::{api_append_messages, api_events, api_file, api_health, api_resolve_project};
use search::api_search;
use sessions::{
    api_attach_session, api_create_session, api_create_workspace, api_delete_session,
    api_delete_workspace, api_list_sessions, api_list_workspaces,
};

mod cards;
mod hooks;
mod search;
mod sessions;

/// The REST routes over `memory` (the memory module's instance) and `host`
/// (where hook events go).
pub fn routes(memory: Arc<Memory>, host: Arc<Host>) -> Router {
    Router::new()
        .route("/health", get(api_health))
        .route(
            "/api/sessions",
            get(api_list_sessions).post(api_create_session),
        )
        .route("/api/sessions/{session_id}", delete(api_delete_session))
        .route(
            "/api/workspaces",
            get(api_list_workspaces).post(api_create_workspace),
        )
        .route(
            "/api/workspaces/{workspace_id}",
            delete(api_delete_workspace),
        )
        .route(
            "/api/workspaces/{workspace_id}/sessions/{session_id}",
            post(api_attach_session),
        )
        .route("/api/projects/resolve", post(api_resolve_project))
        .route("/api/messages", post(api_append_messages))
        .route("/api/events", post(api_events))
        .route("/api/cards", get(api_get_cards))
        .route("/api/cards/user", put(api_put_user_card))
        .route(
            "/api/cards/workspace/{workspace_id}",
            put(api_put_workspace_card),
        )
        .route("/api/file", get(api_file))
        .route("/api/search", post(api_search))
        .with_state(Arc::new(ApiState { memory, host }))
}

/// The one mapping of a failed operation onto an HTTP status and message
/// (ARCHITECTURE rule 5): every capability error goes through its
/// [`Failure`] kind.
fn rest_status(e: impl Into<Failure>) -> (StatusCode, String) {
    let failure = e.into();
    let code = match failure.kind {
        Kind::InvalidArgument => StatusCode::BAD_REQUEST,
        Kind::NotFound => StatusCode::NOT_FOUND,
        Kind::Conflict => StatusCode::CONFLICT,
        Kind::Denied => StatusCode::FORBIDDEN,
        Kind::FailedPrecondition => StatusCode::PRECONDITION_FAILED,
        Kind::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        Kind::Internal => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (code, failure.message)
}

/// Shared state for API handlers
struct ApiState {
    /// The capability services every endpoint calls.
    memory: Arc<Memory>,
    /// The module host hook events go to.
    host: Arc<Host>,
}

#[cfg(test)]
mod tests;
