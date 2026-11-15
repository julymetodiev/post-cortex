// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Models (ARCHITECTURE section 7, the agent): a module of kind
//! `model_provider` serves models ([`ModelProvider`]); the modules in the
//! `agent` and `context_engine` slots reach them through their host handle
//! ([`crate::ModuleHost::model`]), which routes a request to the provider
//! that lists its model. A request is messages of blocks; the answer is a
//! stream of events ending in [`ModelEvent::Done`], or a typed
//! [`ModelFailure`] a provider classifies where it happens.

use std::fmt;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio::sync::mpsc;

use crate::{ModuleError, Scope};

mod background;
mod info;
mod keyed;
mod listing;
mod remote;
mod serve;
mod sse;
mod wire;

pub use background::Listed;
pub use info::{ModelInfoError, check as check_info, checked};
pub use keyed::KeyedProvider;
pub use listing::{Table, unknown};
pub use post_cortex_proto::module::{Modality, ModelFeature, ModelInfo, ModelKind, ModelPrice};
pub use remote::RemoteModels;
pub use serve::{ModelService, ModelSource};
pub use sse::{AnswerReader, Lines, relay};

/// One piece of a message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Block {
    /// Text.
    Text {
        /// The text.
        text: String,
    },
    /// The model's reasoning, as it gave it.
    Thinking {
        /// The reasoning.
        text: String,
        /// The provider's signature of it, when it gives one.
        signature: Option<String>,
    },
    /// The model calls a tool.
    ToolUse {
        /// The call's id, which its result names.
        id: String,
        /// The tool.
        name: String,
        /// Its input.
        input: Value,
    },
    /// What the conversation before it said, in short: the conversation
    /// sent to a model starts from the newest one (a compaction).
    Summary {
        /// The summary.
        text: String,
    },
    /// A tool's result, in the message after the call.
    ToolResult {
        /// The call it answers.
        tool_use_id: String,
        /// Its text.
        content: String,
        /// Whether the tool failed.
        is_error: bool,
    },
    /// How the turn it closes ended (W25): kept in the transcript, never
    /// sent to a model.
    End {
        /// Why it ended (`end_turn`, `cancelled`, `failed`, ...).
        stop: String,
        /// What happened, for a turn blocked, failed or cancelled.
        detail: String,
    },
}

/// Who speaks a message; a tool's result is a block of a user message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// The person, or the results of tools.
    User,
    /// The model.
    Assistant,
}

/// A message of a request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelMessage {
    /// Who speaks it.
    pub role: Role,
    /// Its blocks, in order.
    pub blocks: Vec<Block>,
}

/// A tool the model may call.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelTool {
    /// Its name.
    pub name: String,
    /// What it does.
    pub description: String,
    /// The JSON Schema of its input.
    pub input_schema: Map<String, Value>,
}

/// Whether the model must call a tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolChoice {
    /// The model decides.
    Auto,
    /// The model calls this tool.
    Tool(String),
}

/// What a model is asked.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelRequest {
    /// The model's id, as its provider lists it.
    pub model: String,
    /// The system prompt; empty: none.
    pub system: String,
    /// The conversation, oldest first.
    pub messages: Vec<ModelMessage>,
    /// The tools it may call.
    pub tools: Vec<ModelTool>,
    /// The most tokens it may answer.
    pub max_tokens: u32,
    /// Whether it must call a tool.
    pub tool_choice: ToolChoice,
}

/// The tokens of one answer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// Input tokens not served from the provider's cache.
    pub input: u64,
    /// Output tokens.
    pub output: u64,
    /// Input tokens served from the provider's cache.
    pub cache_read: u64,
    /// Input tokens written to the provider's cache.
    pub cache_write: u64,
}

/// Why a model stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// It finished its answer.
    EndTurn,
    /// It reached the most tokens.
    MaxTokens,
    /// It called tools and waits for their results.
    ToolUse,
}

/// What a model's answer streams.
#[derive(Debug, Clone, PartialEq)]
pub enum ModelEvent {
    /// More of its text.
    TextDelta(String),
    /// More of its reasoning.
    ThinkingDelta(String),
    /// A tool call, whole.
    ToolUse {
        /// The call's id.
        id: String,
        /// The tool.
        name: String,
        /// Its input.
        input: Value,
    },
    /// The answer, whole; the last event.
    Done {
        /// Its blocks.
        blocks: Vec<Block>,
        /// Its tokens.
        usage: Usage,
        /// Why it stopped.
        stop: StopReason,
    },
}

/// What kind of failure a model call met; what the agent does next
/// depends on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelFailureKind {
    /// Too many requests: retry after a while.
    RateLimited,
    /// The provider is overloaded: retry with backoff.
    Overloaded,
    /// The request is longer than the model's context: compact it.
    ContextTooLong,
    /// The credentials were refused.
    Unauthorized,
    /// The account's quota or plan is used up until it resets: retrying
    /// does not help.
    QuotaExhausted,
    /// The provider could not be reached, or failed (network, 5xx).
    Unavailable,
    /// The request was refused as it is; or the answer did not read.
    Invalid,
}

/// A failed model call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelFailure {
    /// Its kind.
    pub kind: ModelFailureKind,
    /// What happened, for a person.
    pub message: String,
    /// How long to wait before a retry, when the provider says.
    pub retry_after: Option<Duration>,
}

impl ModelFailure {
    /// A failure of `kind`.
    #[must_use]
    pub fn new(kind: ModelFailureKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            retry_after: None,
        }
    }

    /// The failure, to be retried after the seconds a `retry-after` header
    /// names (`header`); a header that names no seconds (none, or a date)
    /// says nothing.
    #[must_use]
    pub fn with_retry_after(self, header: Option<&str>) -> Self {
        Self {
            retry_after: header
                .and_then(|v| v.trim().parse::<u64>().ok())
                .map(Duration::from_secs),
            ..self
        }
    }
}

impl ModelFailureKind {
    /// Its name, as messages and the wire give it.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::RateLimited => "rate_limited",
            Self::Overloaded => "overloaded",
            Self::ContextTooLong => "context_too_long",
            Self::Unauthorized => "unauthorized",
            Self::QuotaExhausted => "quota_exhausted",
            Self::Unavailable => "unavailable",
            Self::Invalid => "invalid",
        }
    }
}

impl fmt::Display for ModelFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.kind.name(), self.message)
    }
}

impl std::error::Error for ModelFailure {}

/// An answer being streamed: its events, then its end.
pub struct ModelRun {
    events: mpsc::Receiver<Result<ModelEvent, ModelFailure>>,
}

impl ModelRun {
    /// An answer whose events arrive on `events`.
    #[must_use]
    pub fn new(events: mpsc::Receiver<Result<ModelEvent, ModelFailure>>) -> Self {
        Self { events }
    }

    /// The next event; none after [`ModelEvent::Done`] or a failure.
    pub async fn next(&mut self) -> Option<Result<ModelEvent, ModelFailure>> {
        self.events.recv().await
    }

    /// Wait for the end: the answer's blocks, tokens and stop reason.
    ///
    /// # Errors
    /// The call's failure; `Unavailable` when it ended without an answer.
    pub async fn collect(mut self) -> Result<(Vec<Block>, Usage, StopReason), ModelFailure> {
        while let Some(event) = self.next().await {
            if let ModelEvent::Done {
                blocks,
                usage,
                stop,
            } = event?
            {
                return Ok((blocks, usage, stop));
            }
        }
        Err(ModelFailure::new(
            ModelFailureKind::Unavailable,
            "the answer ended before it was done",
        ))
    }
}

/// The models of a module of kind `model_provider`.
#[async_trait]
pub trait ModelProvider: Send + Sync {
    /// Every model it serves.
    fn models(&self) -> Vec<ModelInfo>;

    /// Its models once the listing it began at start is in (a provider
    /// lists its API's models in the background); [`ModelProvider::models`]
    /// for one that lists nothing.
    ///
    /// # Errors
    /// Its listing was refused (its key, as its field).
    async fn listed(&self) -> Result<Vec<ModelInfo>, ModuleError> {
        Ok(self.models())
    }

    /// Ask `request.model` within `scope`'s deadline and cancellation.
    ///
    /// # Errors
    /// The call failed before its answer started; a failure after that
    /// ends its stream.
    async fn generate(
        &self,
        scope: &Scope,
        request: ModelRequest,
    ) -> Result<ModelRun, ModelFailure>;
}

#[cfg(test)]
mod tests;
