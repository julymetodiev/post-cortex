// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! `get_card` and `update_card`: the project card of a workspace or the
//! user card, versioned.

use crate::Memory;
use post_cortex_core::core::card::CardScope;
use post_cortex_core::services::{GetCardRequest, UpdateCardRequest};

use crate::tools::Failure;
use crate::tools::args::ManageWorkspaceRequest;
use crate::tools::check;
use crate::tools::handlers::Outcome;
use crate::tools::reply;

pub async fn handle(
    memory: &Memory,
    req: &ManageWorkspaceRequest,
    action: &str,
    default_author: Option<String>,
    person: Option<uuid::Uuid>,
) -> Outcome {
    let scope = card_scope(req, person)?;
    if action == "get_card" {
        let response = memory
            .cards
            .get(GetCardRequest {
                scope,
                history: req.history.unwrap_or(0) as usize,
            })
            .await
            .map_err(Failure::from)?;
        let message = match &response.card {
            Some(card) => format!(
                "Card {} version {} ({} bytes)",
                scope.key(),
                card.version,
                card.text.len()
            ),
            None => format!(
                "No card for {} yet (create one with expected_version 0)",
                scope.key()
            ),
        };
        return reply::with(message, &response);
    }
    let content = req.content.clone().ok_or_else(|| {
        Failure::invalid("update_card needs content (the whole new card text)").parameter("content")
    })?;
    let expected_version = req.expected_version.ok_or_else(|| {
        Failure::invalid(
            "update_card needs expected_version: the version you read with get_card (0 for a new card)",
        )
        .parameter("expected_version")
    })?;
    let response = memory
        .cards
        .update(UpdateCardRequest {
            scope,
            text: content,
            expected_version,
            // The caller's own name for itself, else the header's or the
            // client's.
            author: crate::tools::handlers::author(req.author.as_deref(), default_author),
        })
        .await
        .map_err(Failure::from)?;
    let message = match (&response.written, &response.card) {
        (true, Some(card)) => format!("Card {} saved as version {}", scope.key(), card.version),
        (_, Some(card)) => format!(
            "Not saved: the card is at version {} now, not {}. Merge your change into the text below and retry with expected_version {}",
            card.version, expected_version, card.version
        ),
        (_, None) => "Not saved: there is no card yet; retry with expected_version 0".to_string(),
    };
    reply::with(message, &response)
}

/// The card `req` names: the user card (`person`'s, the host's for none),
/// or the project card of its `workspace_id`.
fn card_scope(
    req: &ManageWorkspaceRequest,
    person: Option<uuid::Uuid>,
) -> Result<CardScope, Failure> {
    match req.card.as_deref().map(str::to_lowercase).as_deref() {
        Some("user") => Ok(CardScope::user_of(person)),
        None | Some("workspace") => {
            let raw = req.workspace_id.as_deref().ok_or_else(|| {
                Failure::invalid(
                    "workspace_id is required for a project card (or set card: \"user\")",
                )
                .parameter("workspace_id")
            })?;
            Ok(CardScope::Workspace(check::workspace_id(raw)?))
        }
        Some(other) => Err(Failure::invalid(format!(
            "Invalid card '{other}': expected 'workspace' or 'user'"
        ))
        .parameter("card")),
    }
}
