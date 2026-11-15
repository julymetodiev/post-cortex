// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Turns (ARCHITECTURE section 7, the agent): the module in the `agent`
//! slot runs them ([`Turns`], from [`crate::Module::turns`]); a client of
//! the host asks for one (`Host::turns`). A turn streams its events, then
//! says how it ended. Across a socket a turn is `pcx.agent.v1.Agent`:
//! [`TurnsService`] serves one, [`RemoteTurns`] asks one.

use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::model::{ModelFailure, ModelInfo, StopReason, Usage};
use crate::{ModuleError, Scope};
use serde_json::Value;

mod remote;
mod serve;
#[cfg(test)]
mod serve_tests;
mod wire;

pub use pcx_module_core::TurnGrant;
pub use remote::RemoteTurns;
pub use serve::{TURN_DEADLINE, TurnsService, TurnsSource};

/// One event of a running turn.
#[derive(Debug, Clone, PartialEq)]
pub enum TurnEvent {
    /// More of the answer's text.
    Text(String),
    /// More of the model's reasoning.
    Thinking(String),
    /// The model calls a tool.
    ToolCall {
        /// The call's id.
        id: String,
        /// The tool.
        name: String,
        /// Its input.
        input: Value,
    },
    /// A tool answered.
    ToolResult {
        /// The call it answers.
        id: String,
        /// Its text.
        content: String,
        /// Whether it failed.
        is_error: bool,
    },
    /// A model call failed and is asked again after `after`.
    Retrying {
        /// Why.
        failure: ModelFailure,
        /// The wait.
        after: Duration,
    },
}

/// Why a turn ended.
#[derive(Debug, Clone, PartialEq)]
pub enum TurnStop {
    /// The model answered (finished, or reached its most tokens).
    Answered(StopReason),
    /// A module blocked the turn, with its reason.
    Blocked(String),
    /// The turn took the most steps it may.
    StepLimit,
    /// Its tools kept failing: a model going in circles, with what failed.
    Repeating(String),
    /// It was cancelled.
    Cancelled,
    /// A model call failed past its recovery.
    Failed(ModelFailure),
}

impl TurnStop {
    /// Its wire name (`end_turn`, `max_tokens`, `tool_use`, `blocked`,
    /// `step_limit`, `cancelled`, `failed`).
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::Answered(StopReason::EndTurn) => "end_turn",
            Self::Answered(StopReason::MaxTokens) => "max_tokens",
            Self::Answered(StopReason::ToolUse) => "tool_use",
            Self::Blocked(_) => "blocked",
            Self::StepLimit => "step_limit",
            Self::Repeating(_) => "repeating",
            Self::Cancelled => "cancelled",
            Self::Failed(_) => "failed",
        }
    }

    /// What it says besides its name: a block's reason, a failure's
    /// message; empty for the others.
    #[must_use]
    pub fn detail(&self) -> String {
        match self {
            Self::Blocked(reason) | Self::Repeating(reason) => reason.clone(),
            Self::Failed(failure) => failure.to_string(),
            Self::Answered(_) | Self::StepLimit | Self::Cancelled => String::new(),
        }
    }
}

/// How a turn ended.
#[derive(Debug, Clone, PartialEq)]
pub struct TurnEnd {
    /// Why.
    pub stop: TurnStop,
    /// The model calls it made.
    pub steps: u32,
    /// The tokens of all of them.
    pub usage: Usage,
    /// The text of its last answer; empty when the model gave none.
    pub answer: String,
}

/// What a turn is asked: its session and prompt, and what it adds to the
/// agent's settings (a team member's role and skills, model and tools).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TurnRequest {
    /// The session whose transcript the turn continues.
    pub session: String,
    /// What the person asks.
    pub prompt: String,
    /// Text added to the agent's system prompt; empty: none.
    pub instructions: String,
    /// The model; none: the agent's.
    pub model: Option<String>,
    /// The modules whose tools the turn may use; empty: all the agent may.
    pub modules: Vec<String>,
    /// The tools the turn may use; empty: all of those modules'.
    pub tools: Vec<String>,
    /// The tools it is not given, whatever `modules` and `tools` allow.
    pub withheld: Vec<String>,
    /// A team's member whose turn a client asks of the team (W25); none:
    /// not a member's.
    pub member: Option<String>,
    /// Who the agent's turns are written as in the transcript (W25: the
    /// chief, a team's member); none: the agent.
    pub speaker: Option<String>,
    /// Who asks: the prompt is written as them (W25: the person a module's
    /// turn is for); none: the scope's author.
    pub asker: Option<String>,
    /// The workspace of a call whose scope has none and whose input names
    /// none; none: such a call has none.
    pub workspace: Option<String>,
}

impl TurnRequest {
    /// A turn of `session` for `prompt`, with the agent's settings.
    #[must_use]
    pub fn new(session: impl Into<String>, prompt: impl Into<String>) -> Self {
        Self {
            session: session.into(),
            prompt: prompt.into(),
            ..Self::default()
        }
    }
}

/// The turns of the module in the `agent` slot.
#[async_trait]
pub trait Turns: Send + Sync {
    /// Run the turn `request` asks in `scope` (its author writes what the
    /// turn writes), its events sent to `events` as they happen: a caller
    /// that reads them slowly holds the turn back, one that dropped the
    /// receiver does not stop it (a caller that does not listen drops it at
    /// once); `scope`'s cancellation does.
    ///
    /// # Errors
    /// `FailedPrecondition` before the module runs; its host, models or
    /// transcript failed.
    async fn run(
        &self,
        scope: &Scope,
        request: &TurnRequest,
        events: &mpsc::Sender<TurnEvent>,
    ) -> Result<TurnEnd, ModuleError>;

    /// The models a turn may name: every one of the host's providers.
    ///
    /// # Errors
    /// `FailedPrecondition` for turns that do not choose their model (a
    /// module's own turns, which run the agent's); the agent's host or its
    /// providers failed.
    async fn models(&self) -> Result<Vec<ModelInfo>, ModuleError> {
        Err(ModuleError::new(
            crate::ModuleErrorKind::FailedPrecondition,
            "these turns do not choose a model: the agent's do",
        ))
    }
}
