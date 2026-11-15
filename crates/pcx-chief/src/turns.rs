// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The chief's turns as `RunTurn` asks them (`agent` `chief`): the request's
//! session is the conversation, its prompt the question, the caller who
//! asks; the chief service's turn, its events streamed. No logic of its
//! own.

use async_trait::async_trait;
use pcx_module_api::agent::{TurnEnd, TurnEvent, TurnRequest, Turns};
use pcx_module_api::{ModuleError, ModuleErrorKind, Scope};
use tokio::sync::mpsc;

use crate::service::{Asker, Conversation};
use crate::wiring::{self, Shared, Wired};

/// The chief's turns.
pub struct ChiefTurns {
    /// The running chief.
    pub shared: Shared,
}

fn invalid(reason: &str) -> ModuleError {
    ModuleError::new(ModuleErrorKind::InvalidArgument, reason)
}

#[async_trait]
impl Turns for ChiefTurns {
    async fn run(
        &self,
        scope: &Scope,
        request: &TurnRequest,
        events: &mpsc::Sender<TurnEvent>,
    ) -> Result<TurnEnd, ModuleError> {
        if !request.instructions.is_empty()
            || request.model.is_some()
            || !request.modules.is_empty()
            || !request.tools.is_empty()
            || !request.withheld.is_empty()
        {
            return Err(invalid(
                "a turn of the chief takes its session and prompt only: its settings are a person's",
            ));
        }
        let author = scope
            .author
            .as_deref()
            .ok_or_else(|| invalid("a turn of the chief needs the caller's author: who asks"))?;
        let asker = Asker {
            author,
            channel: scope.caller_module.as_deref().unwrap_or_default(),
        };
        let wired = Wired::new(wiring::running(&self.shared)?, scope).await?;
        let ports = wired.ports();
        let asked = wired
            .chief(&ports)
            .ask(
                asker,
                &request.prompt,
                Conversation::Session(&request.session),
                Some(events),
            )
            .await?;
        Ok(asked.end)
    }
}
