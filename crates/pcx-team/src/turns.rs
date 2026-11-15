// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The team's turns as `RunTurn` asks them (W25: `agent` `team` and the
//! `member` it names): a person's talk with a member, over the member's
//! work in the caller's workspace. No logic of its own.

use async_trait::async_trait;
use pcx_documents::{HostDocuments, HostSessions};
use pcx_module_api::agent::{TurnEnd, TurnEvent, TurnRequest, Turns};
use pcx_module_api::{ModuleError, ModuleErrorKind, Scope};
use tokio::sync::mpsc;

use crate::adapters::HostWorld;
use crate::module::{self, Shared};
use crate::work::Worker;

/// The team's turns.
pub struct TeamTurns {
    /// The running team.
    pub shared: Shared,
}

fn invalid(reason: &str) -> ModuleError {
    ModuleError::new(ModuleErrorKind::InvalidArgument, reason)
}

#[async_trait]
impl Turns for TeamTurns {
    async fn run(
        &self,
        scope: &Scope,
        request: &TurnRequest,
        events: &mpsc::Sender<TurnEvent>,
    ) -> Result<TurnEnd, ModuleError> {
        let member = request
            .member
            .as_deref()
            .ok_or_else(|| invalid("a turn of the team names its member"))?;
        if !request.instructions.is_empty()
            || request.model.is_some()
            || !request.modules.is_empty()
            || !request.tools.is_empty()
            || !request.withheld.is_empty()
        {
            return Err(invalid(
                "a turn of a member takes its session and prompt only: its settings are the team's",
            ));
        }
        let workspace = scope
            .workspace_id
            .as_deref()
            .ok_or_else(|| invalid("a turn of a member runs in a workspace"))?;
        let running = module::running(&self.shared)?;
        let turns = running.host.turns().await?;
        let world = HostWorld::new(running.channel.clone(), scope, workspace);
        let docs = HostDocuments::new(running.channel.clone(), scope, workspace);
        let sessions = HostSessions::new(running.channel.clone(), scope, workspace);
        Worker {
            docs: &docs,
            board: &world,
            library: &world,
            sessions: &sessions,
            turns: turns.as_ref(),
            workplace: &world,
            scope,
        }
        .talk(member, &request.session, &request.prompt, events)
        .await
    }
}
