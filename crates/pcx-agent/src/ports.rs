// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What the agent service needs of the world (ARCHITECTURE section 7, the
//! agent) besides its models (`pcx_module_api::model::ModelProvider`): the
//! session's transcript, the tools it may call, and the modules that answer
//! its events. Its adapters fill them over the host handle; tests fill them
//! in memory.

use async_trait::async_trait;
use pcx_module_api::model::ModelTool;
use pcx_module_api::{Event, EventKind, ModuleError, Outcome};
use serde_json::Value;

use crate::domain::Turn;

/// A session's turns.
#[async_trait]
pub trait Transcript: Send + Sync {
    /// Every turn of `session`, oldest first.
    ///
    /// # Errors
    /// Memory refused or failed.
    async fn load(&self, session: &str) -> Result<Vec<Turn>, ModuleError>;

    /// Keep `turns` at the end of `session`'s stream, in order.
    ///
    /// # Errors
    /// Memory refused or failed.
    async fn append(&self, session: &str, turns: &[Turn]) -> Result<(), ModuleError>;
}

/// What a tool answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolAnswer {
    /// Its text, as the model reads it.
    pub content: String,
    /// Whether it failed.
    pub is_error: bool,
    /// Whether it failed because its module runs on a runner that is not
    /// connected: nothing the model can do, the work waits for it.
    pub offline: bool,
}

/// The tools the agent may call.
#[async_trait]
pub trait Tools: Send + Sync {
    /// The tools, as the model is told them, each with its module.
    ///
    /// # Errors
    /// The host cannot be reached.
    async fn list(&self) -> Result<Vec<(String, ModelTool)>, ModuleError>;

    /// Call `name` with `input`; a tool's failure is its answer (the model
    /// reads it), not an error of the turn.
    async fn call(&self, name: &str, input: Value) -> ToolAnswer;
}

/// The modules that answer the agent's events.
#[async_trait]
pub trait Hooks: Send + Sync {
    /// Send `event` of `kind`; the merged outcome.
    ///
    /// # Errors
    /// The host cannot be reached.
    async fn send(&self, kind: EventKind, event: Event) -> Result<Outcome, ModuleError>;
}
