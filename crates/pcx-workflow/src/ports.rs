// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What the workflow module needs of the world (docs/modules/workflow.md,
//! layers): its definitions, runs and state (`Workflows`, `Runs`, `State`,
//! over memory's documents), the tools a scope reaches and calls (`Tools`,
//! over the host channel), the models and the agent's turns (`Models`,
//! `Turns`), the bus (`Events`), memory's blobs for large outputs
//! (`Blobs`) and the time (`Clock`). Its adapters fill them; tests fill
//! them in memory.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use pcx_module_api::bus::EventFeed;
use pcx_module_api::{ModuleError, Scope};
use post_cortex_proto::module::WorkflowTemplate;
use post_cortex_proto::workflow::{Run, StateEntry, Step, Workflow};
use prost_reflect::DynamicMessage;
use serde_json::Value as Json;

use crate::domain::Tool;

/// A document as it is kept: its id and the version read.
#[derive(Debug, Clone, PartialEq)]
pub struct Kept<T> {
    /// Its document's id.
    pub id: String,
    /// The version read.
    pub version: u64,
    /// What it holds.
    pub data: T,
}

/// The workflows of the module's workspace.
#[async_trait]
pub trait Workflows: Send + Sync {
    /// The workflow named `name`, at its latest version.
    ///
    /// # Errors
    /// `Conflict` when two have the name; memory failed.
    async fn named(&self, name: &str) -> Result<Option<Kept<Workflow>>, ModuleError>;

    /// Workflow `id`, at its latest version.
    ///
    /// # Errors
    /// Memory failed.
    async fn get(&self, id: &str) -> Result<Option<Kept<Workflow>>, ModuleError>;

    /// Workflow `id` at `version`: what a run pinned to it runs.
    ///
    /// # Errors
    /// Memory failed.
    async fn at(&self, id: &str, version: u64) -> Result<Option<Kept<Workflow>>, ModuleError>;

    /// Every workflow, at its latest version.
    ///
    /// # Errors
    /// Memory failed.
    async fn all(&self) -> Result<Vec<Kept<Workflow>>, ModuleError>;

    /// `workflow` kept as a new one.
    ///
    /// # Errors
    /// Memory failed.
    async fn create(&self, workflow: &Workflow) -> Result<Kept<Workflow>, ModuleError>;

    /// Workflow `id` changed to `workflow` from version `version`: its next
    /// version.
    ///
    /// # Errors
    /// `NotFound` for none; `Conflict` when it changed since `version`;
    /// memory failed.
    async fn change(
        &self,
        id: &str,
        version: u64,
        workflow: &Workflow,
    ) -> Result<Kept<Workflow>, ModuleError>;

    /// Workflow `id` taken out: a new one whose grants could not be asked.
    ///
    /// # Errors
    /// Memory failed.
    async fn delete(&self, id: &str) -> Result<(), ModuleError>;
}

/// The runs of the module's workflows and their journals.
#[async_trait]
pub trait Runs: Send + Sync {
    /// `run` kept as a new one.
    ///
    /// # Errors
    /// Memory failed.
    async fn create(&self, run: &Run) -> Result<Kept<Run>, ModuleError>;

    /// Run `id` changed to `run` from version `version`.
    ///
    /// # Errors
    /// `NotFound`; `Conflict` when it changed since; memory failed.
    async fn change(&self, id: &str, version: u64, run: &Run) -> Result<Kept<Run>, ModuleError>;

    /// Run `id`.
    ///
    /// # Errors
    /// Memory failed.
    async fn get(&self, id: &str) -> Result<Option<Kept<Run>>, ModuleError>;

    /// The runs of workflow `workflow`, the most recently changed first, at
    /// most `limit`: read only as far as that.
    ///
    /// # Errors
    /// Memory failed.
    async fn of(&self, workflow: &str, limit: usize) -> Result<Vec<Kept<Run>>, ModuleError>;

    /// The runs of workflow `workflow` that have ended, the most recently
    /// changed first, at most `count`: read only as far as that.
    ///
    /// # Errors
    /// Memory failed.
    async fn ended(&self, workflow: &str, count: usize) -> Result<Vec<Kept<Run>>, ModuleError>;

    /// The runs that have not ended (running or waiting): what goes on
    /// after a restart.
    ///
    /// # Errors
    /// Memory failed.
    async fn open(&self) -> Result<Vec<Kept<Run>>, ModuleError>;

    /// The steps of run `run`.
    ///
    /// # Errors
    /// Memory failed.
    async fn steps(&self, run: &str) -> Result<Vec<Kept<Step>>, ModuleError>;

    /// `step` kept as a new one.
    ///
    /// # Errors
    /// Memory failed.
    async fn put_step(&self, step: &Step) -> Result<Kept<Step>, ModuleError>;

    /// Step `id` changed to `step` from version `version`.
    ///
    /// # Errors
    /// `NotFound`; `Conflict`; memory failed.
    async fn change_step(
        &self,
        id: &str,
        version: u64,
        step: &Step,
    ) -> Result<Kept<Step>, ModuleError>;
}

/// The workflows' key-value stores.
#[async_trait]
pub trait State: Send + Sync {
    /// The value of `key` in `workflow`'s store.
    ///
    /// # Errors
    /// Memory failed.
    async fn get(&self, workflow: &str, key: &str)
    -> Result<Option<Kept<StateEntry>>, ModuleError>;

    /// `entry` written: a new one (`read` none) or from the one read.
    ///
    /// # Errors
    /// `Conflict` when it changed since it was read (or another was
    /// written meanwhile); memory failed.
    async fn put(&self, entry: &StateEntry, read: Option<(&str, u64)>) -> Result<(), ModuleError>;

    /// Entry `id` removed.
    ///
    /// # Errors
    /// Memory failed.
    async fn delete(&self, id: &str) -> Result<(), ModuleError>;
}

/// The tools of running modules a scope may call.
#[async_trait]
pub trait Tools: Send + Sync {
    /// The tools `scope` may call, by their gRPC paths.
    ///
    /// # Errors
    /// `Unavailable` when the host cannot be reached; a module's tools that
    /// do not read.
    async fn reachable(&self, scope: &Scope) -> Result<Vec<(String, Tool)>, ModuleError>;

    /// Call the tool at `path` in `scope` with `request`, carrying
    /// `idempotency_key` when it must act once: its response.
    ///
    /// # Errors
    /// `Denied` when the scope may not call it; the call's failure.
    async fn call(
        &self,
        scope: &Scope,
        path: &str,
        request: &DynamicMessage,
        idempotency_key: Option<&str>,
    ) -> Result<DynamicMessage, ModuleError>;

    /// The workflow templates of the running modules whose tools `scope`
    /// may call, each with its module, in their load order.
    ///
    /// # Errors
    /// `Unavailable` when the host cannot be reached.
    async fn templates(
        &self,
        scope: &Scope,
    ) -> Result<Vec<(String, WorkflowTemplate)>, ModuleError>;
}

/// A model's answer through its forced tool.
#[derive(Debug, Clone, PartialEq)]
pub struct Answer {
    /// The tool's input: the answer.
    pub answer: Json,
    /// The tokens it spent, in and out.
    pub tokens: u64,
}

/// The host's models.
#[async_trait]
pub trait Models: Send + Sync {
    /// Ask `model` (empty: the host's default) with `system` and `prompt`,
    /// its answer the input of one forced tool of input schema `schema`.
    ///
    /// # Errors
    /// `Denied` without the capability; the model failed or answered no
    /// call of the tool.
    async fn answer(
        &self,
        scope: &Scope,
        model: &str,
        system: &str,
        prompt: &str,
        schema: &Json,
    ) -> Result<Answer, ModuleError>;
}

/// A turn's end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Said {
    /// The text of its last answer.
    pub answer: String,
    /// The tokens it spent.
    pub tokens: u64,
}

/// Whose turn an `agent` block asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Whose<'a> {
    /// A team member's, by its name: the team runs it with the member's
    /// own role, skills, model, modules and tools.
    Member(&'a str),
    /// The host's agent's, with the tools of these modules and no other:
    /// the modules the workflow's grants name.
    Agent(&'a [String]),
}

/// The turns a workflow asks.
#[async_trait]
pub trait Turns: Send + Sync {
    /// A turn of `whose` in `session` for `prompt`.
    ///
    /// # Errors
    /// No such member, or one that is not an agent of the harness; the
    /// agent's failure.
    async fn turn(
        &self,
        scope: &Scope,
        session: &str,
        whose: Whose<'_>,
        prompt: &str,
    ) -> Result<Said, ModuleError>;

    /// Whether a turn of `whose` can be asked now: the agent runs and has a
    /// model, and a member is one of the workspace's teams. A workflow with
    /// `agent` blocks is written only then (docs/modules/workflow.md,
    /// triggers and approval).
    ///
    /// # Errors
    /// `FailedPrecondition` naming what is missing and where a person adds
    /// it; the host or the team failed.
    async fn ready(&self, whose: Whose<'_>) -> Result<(), ModuleError>;

    /// A new session for a run's turns, named `name`; its id.
    ///
    /// # Errors
    /// Memory refused or failed.
    async fn session(&self, name: &str) -> Result<String, ModuleError>;
}

/// The organisation's bus as the module sees it.
#[async_trait]
pub trait Events: Send + Sync {
    /// Emit `name` with `payload` in `scope`.
    ///
    /// # Errors
    /// The host refused or failed.
    async fn publish(
        &self,
        scope: &Scope,
        name: &str,
        payload: pbjson_types::Any,
    ) -> Result<(), ModuleError>;

    /// The events the module follows, after what it acknowledged.
    ///
    /// # Errors
    /// The host keeps no events.
    async fn follow(&self) -> Result<EventFeed, ModuleError>;

    /// The events named `name` up to `position` are handled.
    ///
    /// # Errors
    /// The host refused or failed.
    async fn ack(&self, name: &str, position: u64) -> Result<(), ModuleError>;
}

/// Memory's blobs of the module's workspace.
#[async_trait]
pub trait Blobs: Send + Sync {
    /// `bytes`, a message, kept: their SHA-256.
    ///
    /// # Errors
    /// Memory refused or failed.
    async fn put(&self, bytes: Vec<u8>) -> Result<String, ModuleError>;

    /// The bytes of blob `sha256`.
    ///
    /// # Errors
    /// `NotFound`; memory failed.
    async fn get(&self, sha256: &str) -> Result<Vec<u8>, ModuleError>;
}

/// The stores of one dry run: its runs and journal held in memory, the
/// workflow it runs over the kept ones, the kept store and blobs under its
/// own writes (docs/modules/workflow.md, triggers and approval).
pub struct DryPorts {
    /// The kept workflows and the one it runs.
    pub workflows: Arc<dyn Workflows>,
    /// Its runs and journal.
    pub runs: Arc<dyn Runs>,
    /// The stores.
    pub state: Arc<dyn State>,
    /// The blobs.
    pub blobs: Arc<dyn Blobs>,
}

/// What a dry run stores in instead of memory.
pub trait Dry: Send + Sync {
    /// Fresh stores for one dry run of `workflow`.
    ///
    /// # Errors
    /// They could not be made.
    fn ports(&self, workflow: Kept<Workflow>) -> Result<DryPorts, ModuleError>;
}

/// The time.
pub trait Clock: Send + Sync {
    /// Now.
    fn now(&self) -> DateTime<Utc>;
}
