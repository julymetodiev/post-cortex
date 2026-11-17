// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! The search itself (embeddings builds).

use crate::Memory;
use crate::capabilities::{Found, SearchScope};
use crate::content_vectorizer::SearchOptions;
use serde_json::Value;
use uuid::Uuid;

use crate::tools::args::SemanticSearchRequest;
use crate::tools::check;
use crate::tools::handlers::Outcome;
use crate::tools::handlers::search::HINTS;
use crate::tools::operations::RERANKED;
use crate::tools::parse::arguments;
use crate::tools::reply;
use crate::tools::responses::SearchResponse;
use crate::tools::select::Select;
use crate::tools::{Failure, wire};

mod text;

#[cfg(test)]
mod tests;

/// About how many characters of a long result's text a snippet shows.
const SNIPPET_CHARS: usize = 400;

pub async fn handle(memory: &Memory, params: Value) -> Outcome {
    let req: SemanticSearchRequest = arguments(params, HINTS)?;
    let scope = req.scope.as_deref().unwrap_or("global").to_lowercase();
    check::scope(&scope)?;
    let limit = check::limit("limit", req.limit, 10, 1000)?;
    let select = Select::new(req.select.clone(), req.select_map.clone());

    let target = match scope.as_str() {
        "session" => SearchScope::Session(check::session_id(scope_id(&req, "session")?)?),
        "workspace" => {
            let raw = scope_id(&req, "workspace")?;
            SearchScope::Workspace(Uuid::parse_str(raw).map_err(|_| {
                check::invalid(
                    "Invalid UUID: Invalid UUID format",
                    "scope_id",
                    check::Problem::BadUuid,
                    None,
                )
                .expected("Valid UUID string")
            })?)
        }
        _ => SearchScope::Global,
    };

    if let SearchScope::Workspace(ws) = target {
        // An empty workspace answers with a note of its own.
        let workspace = memory.workspaces.get(ws).map_err(Failure::from)?;
        if workspace.session_count() == 0 {
            let mut response = SearchResponse::new(&req.query, &[], false, None);
            response.workspace_id = Some(ws.to_string());
            response.note = Some("workspace has no sessions");
            return reranked(
                reply::selected(
                    format!("Found 0 results (workspace {ws} has no sessions)"),
                    &response,
                    &select,
                ),
                false,
            );
        }
    }

    let snippets = match req.detail.as_deref() {
        None | Some("snippet") => true,
        Some("full") => false,
        Some(other) => {
            return Err(
                Failure::invalid(format!("detail is snippet or full, not {other}"))
                    .parameter("detail"),
            );
        }
    };
    let options = SearchOptions {
        limit: Some(limit),
        date_range: wire::date_range(req.date_from.as_deref(), req.date_to.as_deref())?,
        recency_bias: check::recency_bias(req.recency_bias)?,
        mode: wire::mode(req.mode.as_deref())?,
        rerank_withheld: wire::rerank_withheld(req.rerank_withheld.as_deref())?,
        corpus: wire::sources(req.sources.as_deref())?,
        author: req.author.clone().filter(|a| !a.trim().is_empty()),
        prefer_author: req.prefer_author.clone().filter(|a| !a.trim().is_empty()),
        trace: req.trace.unwrap_or(false),
        update_types: req
            .interaction_type
            .iter()
            .flatten()
            .map(|t| wire::parse("interaction_type", t))
            .collect::<Result<_, _>>()?,
        // A snippet search shows the hits it found, as passages; a full one
        // their whole texts and the records around each.
        passage: snippets.then_some(SNIPPET_CHARS),
        without_neighbours: snippets,
        ..Default::default()
    };
    let found = memory
        .search
        .find(target.clone(), &req.query, options)
        .await
        .map_err(|e| {
            let label = match target {
                SearchScope::Session(_) => "Session semantic search failed",
                SearchScope::Global => "Global semantic search failed",
                _ => "Search failed",
            };
            let message = format!("{label}: {e}");
            Failure::from(e).with_message(message)
        })?;
    reply(&target, &req.query, &found, &select)
}

/// The `scope_id` a scope needs.
fn scope_id<'a>(req: &'a SemanticSearchRequest, scope: &str) -> Result<&'a str, Failure> {
    req.scope_id.as_deref().ok_or_else(|| {
        check::missing(
            "scope_id",
            "UUID string",
            &format!("When scope is '{scope}', you must provide scope_id (the {scope} UUID)"),
        )
    })
}

fn reply(target: &SearchScope, query: &str, found: &Found, select: &Select) -> Outcome {
    let results = found.hits.as_slice();
    let mut response = SearchResponse::new(query, results, found.reranked, found.rerank_skipped);
    let mut message = match target {
        SearchScope::Session(id) => {
            response.session_id = Some(id.to_string());
            text::results_message(
                results,
                format!("Found {} results in session {id}", results.len()),
                false,
            )
        }
        SearchScope::Workspace(id) => {
            response.workspace_id = Some(id.to_string());
            format!("Found {} results", results.len())
        }
        _ => text::results_message(
            results,
            format!("Found {} results across all sessions", results.len()),
            true,
        ),
    };
    if let Some(skipped) = found.rerank_skipped {
        message = format!(
            "{message}\nNot reranked ({}): {}.",
            skipped.as_str(),
            skipped.reason()
        );
    }
    reranked(reply::selected(message, &response, select), found.reranked)
}

/// `outcome` with whether a reranker ordered its hits in its meta, which
/// its `select` does not take away (the cloud counts reranker calls by it).
fn reranked(outcome: Outcome, reranked: bool) -> Outcome {
    let mut output = outcome?;
    output.meta = Some(serde_json::Map::from_iter([(
        RERANKED.to_string(),
        Value::Bool(reranked),
    )]));
    Ok(output)
}
