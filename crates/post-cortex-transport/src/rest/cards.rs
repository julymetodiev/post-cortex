// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! REST routes for the user and project cards.

use crate::rest::ApiState;
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use post_cortex_core::core::card::{Card, CardScope};
use post_cortex_core::services::{GetCardRequest, UpdateCardRequest, UpdateCardResponse};
use post_cortex_memory::capabilities::CardError;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

/// Query of `GET /api/cards`.
#[derive(Deserialize)]
pub struct CardsQuery {
    /// The project whose card to return with the user card.
    workspace_id: Option<Uuid>,
}

/// Body of `GET /api/cards`: the cards a session starts with.
#[derive(Serialize)]
pub struct CardsBody {
    user: Option<Card>,
    workspace: Option<Card>,
}

/// Body of `PUT /api/cards/...`.
#[derive(Deserialize)]
pub struct PutCardBody {
    content: String,
    /// Version the content was edited from (0: no card yet).
    expected_version: u64,
    #[serde(default)]
    author: Option<String>,
}

/// The user card (the request's person's) and, with `workspace_id`, the
/// project card; read by the SessionStart hook and `pcx card`.
pub async fn api_get_cards(
    State(state): State<Arc<ApiState>>,
    headers: axum::http::HeaderMap,
    Query(query): Query<CardsQuery>,
) -> Result<Json<CardsBody>, (StatusCode, String)> {
    let person = crate::person::of_headers(&headers)?;
    let card = |scope| {
        let state = Arc::clone(&state);
        async move {
            state
                .memory
                .cards
                .get(GetCardRequest { scope, history: 0 })
                .await
                .map(|r| r.card)
                .map_err(card_error)
        }
    };
    let user = card(CardScope::user_of(person)).await?;
    let workspace = match query.workspace_id {
        Some(id) => card(CardScope::Workspace(id)).await?,
        None => None,
    };
    Ok(Json(CardsBody { user, workspace }))
}

pub async fn api_put_user_card(
    State(state): State<Arc<ApiState>>,
    headers: axum::http::HeaderMap,
    Json(body): Json<PutCardBody>,
) -> Result<(StatusCode, Json<UpdateCardResponse>), (StatusCode, String)> {
    let person = crate::person::of_headers(&headers)?;
    put_card(&state, CardScope::user_of(person), body, &headers).await
}

pub async fn api_put_workspace_card(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<Uuid>,
    headers: axum::http::HeaderMap,
    Json(body): Json<PutCardBody>,
) -> Result<(StatusCode, Json<UpdateCardResponse>), (StatusCode, String)> {
    put_card(&state, CardScope::Workspace(id), body, &headers).await
}

/// 200 with the new version, or 409 with the latest one when the card
/// changed since `expected_version`. The author is the body's, else the
/// `X-PCX-Author` header's.
pub async fn put_card(
    state: &ApiState,
    scope: CardScope,
    body: PutCardBody,
    headers: &axum::http::HeaderMap,
) -> Result<(StatusCode, Json<UpdateCardResponse>), (StatusCode, String)> {
    let response = state
        .memory
        .cards
        .update(UpdateCardRequest {
            scope,
            text: body.content,
            expected_version: body.expected_version,
            author: body
                .author
                .map(|a| a.trim().to_string())
                .filter(|a| !a.is_empty())
                .or_else(|| crate::mcp::header_author(headers)),
        })
        .await
        .map_err(card_error)?;
    let status = if response.written {
        StatusCode::OK
    } else {
        StatusCode::CONFLICT
    };
    Ok((status, Json(response)))
}

fn card_error(e: CardError) -> (StatusCode, String) {
    crate::rest::rest_status(e)
}
