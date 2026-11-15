// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The REST search route.

use crate::rest::ApiState;
use axum::{Json, extract::State, http::StatusCode};
use pcx_module_api::text::clip;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

/// Body of `POST /api/search`.
#[derive(Deserialize)]
pub struct SearchBody {
    query: String,
    /// `session`, `workspace` or `global`.
    scope: String,
    /// The session or workspace; ignored for `global`, as in MCP.
    #[serde(default)]
    id: Option<Uuid>,
    /// `fast`, `local` or `accurate` (default: the daemon's).
    #[serde(default)]
    mode: Option<String>,
    /// `records` (default), `messages` or `all`.
    #[serde(default)]
    sources: Option<String>,
    /// 1 to 50 (default 5).
    #[serde(default)]
    limit: Option<usize>,
    /// Only records and messages by this author.
    #[serde(default)]
    author: Option<String>,
}

/// One hit of `POST /api/search`.
#[derive(Serialize)]
pub struct SearchHitBody {
    /// Entry id of a record, `message:<seq>` of a captured turn.
    id: String,
    content_type: String,
    /// The record's title; the first line of other hits.
    title: String,
    snippet: String,
    score: f32,
    timestamp: chrono::DateTime<chrono::Utc>,
    author: Option<String>,
    /// The hit's session; none for a document.
    #[serde(skip_serializing_if = "Option::is_none")]
    session_id: Option<Uuid>,
    /// A document's workspace.
    #[serde(skip_serializing_if = "Option::is_none")]
    workspace_id: Option<Uuid>,
}

#[derive(Serialize)]
pub struct SearchResponseBody {
    results: Vec<SearchHitBody>,
    /// Whether a reranker ordered them.
    reranked: bool,
}

pub const SEARCH_TITLE_CHARS: usize = 120;

pub const SEARCH_SNIPPET_CHARS: usize = 300;

/// `POST /api/search`: semantic search over a session, a workspace or
/// everything, for plugins that recall memory without MCP. The scopes run
/// the memory system searches the MCP `semantic_search` tool and gRPC
/// `SemanticSearch` run, with the same options; neighbouring records the
/// search adds around each hit are left out, only hits are returned.
pub async fn api_search(
    State(state): State<Arc<ApiState>>,
    body: Result<Json<SearchBody>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<SearchResponseBody>, (StatusCode, String)> {
    use post_cortex_memory::capabilities::SearchScope;
    use post_cortex_memory::content_vectorizer::SearchOptions;

    let bad = |e: String| (StatusCode::BAD_REQUEST, e);
    let Json(body) = body.map_err(|e| bad(e.body_text()))?;
    let scope = post_cortex_memory::tools::wire::scope(&body.scope).map_err(|f| bad(f.message))?;
    let limit = post_cortex_memory::tools::wire::limit("limit", body.limit, 5, 50)
        .map_err(|f| bad(f.message))?;
    let memory = &state.memory;
    let options = SearchOptions {
        limit: Some(limit),
        mode: post_cortex_memory::tools::wire::mode(body.mode.as_deref())
            .map_err(|f| bad(f.message))?,
        corpus: post_cortex_memory::tools::wire::sources(body.sources.as_deref())
            .map_err(|f| bad(f.message))?,
        author: body.author.filter(|a| !a.trim().is_empty()),
        ..Default::default()
    };
    let id = || {
        body.id
            .ok_or_else(|| bad(format!("id is required for scope '{scope}'")))
    };
    let search_scope = match scope.as_str() {
        "session" => SearchScope::Session(id()?),
        "workspace" => SearchScope::Workspace(id()?),
        _ => SearchScope::Global,
    };
    let found = memory
        .search
        .find(search_scope, &body.query, options)
        .await
        .map_err(crate::rest::rest_status)?;

    let hits = found
        .hits
        .into_iter()
        .filter(|r| r.neighbour_of.is_none())
        .map(|r| {
            let text = r.text_content.trim();
            let (title, snippet) = match &r.title {
                Some(title) => {
                    let rest = text.strip_prefix(title.as_str()).unwrap_or(text).trim();
                    (title.clone(), rest)
                }
                None => (text.lines().next().unwrap_or(text).to_string(), text),
            };
            SearchHitBody {
                id: r.content_id.clone(),
                content_type: r.content_type.as_str().to_string(),
                title: clip(&title, SEARCH_TITLE_CHARS),
                snippet: clip(snippet, SEARCH_SNIPPET_CHARS),
                score: r.combined_score,
                timestamp: r.timestamp,
                author: r.author.clone(),
                session_id: r.source.session(),
                workspace_id: r.source.workspace(),
            }
        })
        .collect();
    Ok(Json(SearchResponseBody {
        results: hits,
        reranked: found.reranked,
    }))
}
