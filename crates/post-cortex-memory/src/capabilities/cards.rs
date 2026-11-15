// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Cards: the project (workspace) and user cards, versioned
//! (ARCHITECTURE section 4).

use std::sync::Arc;

use post_cortex_core::core::card::{Card, CardScope, MAX_CARD_BYTES};
use post_cortex_core::redact::{RedactionCounts, Redactor};
use post_cortex_core::services::{
    GetCardRequest, GetCardResponse, UpdateCardRequest, UpdateCardResponse,
};
use post_cortex_ports::{CardStore, CardWrite};
use uuid::Uuid;

use crate::memory_system::{CardChange, Change, ChangeLog, WorkspaceManager};

/// Why a card operation failed.
#[derive(Debug, thiserror::Error)]
pub enum CardError {
    /// A workspace card of a workspace that does not exist.
    #[error("Workspace {0} not found")]
    WorkspaceNotFound(Uuid),
    /// The card text is too long.
    #[error("Invalid argument: {0}")]
    InvalidArgument(String),
    /// Storage failed.
    #[error("Internal error: {0}")]
    Storage(#[from] crate::memory_system::RuntimeError),
}

/// The Cards capability service.
#[derive(Clone)]
pub struct Cards {
    cards: Arc<dyn CardStore>,
    workspaces: Arc<WorkspaceManager>,
    redactor: Arc<Redactor>,
    changes: ChangeLog,
}

impl Cards {
    pub(crate) fn new(
        cards: Arc<dyn CardStore>,
        workspaces: Arc<WorkspaceManager>,
        redactor: Arc<Redactor>,
        changes: ChangeLog,
    ) -> Self {
        Self {
            cards,
            workspaces,
            redactor,
            changes,
        }
    }

    /// The latest version of a card and, if asked for, older versions.
    ///
    /// # Errors
    /// `WorkspaceNotFound`, or storage failed.
    #[tracing::instrument(skip(self, req), fields(scope = %req.scope.key()), name = "post_cortex.get_card")]
    pub async fn get(&self, req: GetCardRequest) -> Result<GetCardResponse, CardError> {
        self.require_scope(req.scope)?;
        if req.history == 0 {
            return Ok(GetCardResponse {
                card: self.cards.get_card(req.scope).await.map_err(storage)?,
                history: Vec::new(),
            });
        }
        let mut versions = self
            .cards
            .card_history(req.scope, req.history + 1)
            .await
            .map_err(storage)?
            .into_iter();
        Ok(GetCardResponse {
            card: versions.next(),
            history: versions.collect(),
        })
    }

    /// Write the next version of a card (trailing whitespace trimmed,
    /// redacted, at most `MAX_CARD_BYTES`) unless it changed since
    /// `expected_version`; a conflict is a normal response carrying the
    /// latest version.
    ///
    /// # Errors
    /// `WorkspaceNotFound`, `InvalidArgument` for a card that is too long,
    /// or storage failed.
    #[tracing::instrument(
        skip(self, req),
        fields(scope = %req.scope.key(), expected_version = req.expected_version),
        name = "post_cortex.update_card",
    )]
    pub async fn update(&self, req: UpdateCardRequest) -> Result<UpdateCardResponse, CardError> {
        self.require_scope(req.scope)?;
        let mut counts = RedactionCounts::new();
        let text = self.redactor.redact_str(req.text.trim_end(), &mut counts);
        if !counts.is_empty() {
            tracing::info!(scope = %req.scope.key(), ?counts, "update_card: redacted sensitive values");
        }
        if text.len() > MAX_CARD_BYTES {
            return Err(CardError::InvalidArgument(format!(
                "update_card: the card is {} bytes, the limit is {MAX_CARD_BYTES}; shorten it",
                text.len()
            )));
        }
        let write = self
            .cards
            .put_card(req.scope, text, req.author, req.expected_version)
            .await
            .map_err(storage)?;
        Ok(match write {
            CardWrite::Written(card) => {
                self.changes.publish(Change::Card(CardChange {
                    scope: card.scope,
                    version: card.version,
                    at: card.updated_at,
                }));
                UpdateCardResponse {
                    written: true,
                    card: Some(card),
                }
            }
            CardWrite::Conflict(latest) => UpdateCardResponse {
                written: false,
                card: latest,
            },
        })
    }

    /// The user card of `person` (the host's for none) and the cards of
    /// `workspaces`, in that order; cards that cannot be read are left out
    /// (logged). Assembly puts them first.
    pub(crate) async fn for_assembly(
        &self,
        person: Option<Uuid>,
        workspaces: &[Uuid],
    ) -> Vec<Card> {
        let scopes = std::iter::once(CardScope::user_of(person))
            .chain(workspaces.iter().copied().map(CardScope::Workspace));
        let read = futures::future::join_all(
            scopes.map(|scope| async move { (scope, self.cards.get_card(scope).await) }),
        )
        .await;
        let mut cards = Vec::new();
        for (scope, card) in read {
            match card {
                Ok(Some(card)) => cards.push(card),
                Ok(None) => {}
                Err(e) => tracing::warn!("Card {} left out of assembled context: {e}", scope.key()),
            }
        }
        cards
    }

    /// A workspace card needs an existing workspace.
    fn require_scope(&self, scope: CardScope) -> Result<(), CardError> {
        match scope {
            CardScope::Workspace(id) if self.workspaces.get_workspace(&id).is_none() => {
                Err(CardError::WorkspaceNotFound(id))
            }
            _ => Ok(()),
        }
    }
}

/// The one mapping of a storage port error to this capability's error.
fn storage(e: post_cortex_ports::PortError) -> CardError {
    CardError::Storage(e.into())
}

#[cfg(test)]
mod tests;
