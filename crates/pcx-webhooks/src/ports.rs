// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What the webhooks need of the world (ARCHITECTURE section 8,
//! operations): the board, in a source's workspace and as the source,
//! memory, for the incident a resolved group leaves, and the channel, to
//! tell people of a new incident at once, and the workflows, for a source
//! that runs one. The module's adapters
//! fill them over its host channel; tests fill them in memory.

use async_trait::async_trait;
use pcx_module_api::ModuleError;
use post_cortex_proto::board::{AddTaskRequest, BoardRecord, TaskRecord, Thread};
use post_cortex_proto::webhooks::Source;

use crate::domain::Incident;

/// The board, for the webhooks.
#[async_trait]
pub trait Board: Send + Sync {
    /// Add `task` (with its key: the key's task when the board has it).
    ///
    /// # Errors
    /// The board refused or failed.
    async fn add(&self, task: AddTaskRequest) -> Result<TaskRecord, ModuleError>;

    /// The tasks of board `board`.
    ///
    /// # Errors
    /// The board failed.
    async fn tasks(&self, board: &str) -> Result<Vec<TaskRecord>, ModuleError>;

    /// Board `board`, every column with its stage.
    ///
    /// # Errors
    /// No such board; the board failed.
    async fn board(&self, board: &str) -> Result<BoardRecord, ModuleError>;

    /// Task `task` with its comments and decisions.
    ///
    /// # Errors
    /// No such task; the board failed.
    async fn thread(&self, task: &str) -> Result<Thread, ModuleError>;

    /// Comment `text` on task `task`.
    ///
    /// # Errors
    /// The board refused or failed.
    async fn comment(&self, task: &str, text: &str) -> Result<(), ModuleError>;

    /// Move task `task` from its version `version` to the end of `column`,
    /// as `member` (the task's assignee, whose finding the move carries
    /// out; the source is no member of a team, and the board lets only a
    /// member close).
    ///
    /// # Errors
    /// The board refused (a decision holds it, a stale version, a member
    /// who may not close) or failed.
    async fn close(
        &self,
        task: &str,
        version: u64,
        column: &str,
        member: &str,
    ) -> Result<(), ModuleError>;
}

/// Memory, for incidents.
#[async_trait]
pub trait Incidents: Send + Sync {
    /// Record `incident` in session `session` as a `problem_solved` record.
    ///
    /// # Errors
    /// Memory refused or failed.
    async fn record(&self, session: &str, incident: &Incident) -> Result<(), ModuleError>;
}

/// The channel, for telling people at once (W29).
#[async_trait]
pub trait Teller: Send + Sync {
    /// Send `text` to channel destination `destination`.
    ///
    /// # Errors
    /// No channel runs, or it refused or failed.
    async fn tell(&self, destination: &str, text: &str) -> Result<(), ModuleError>;
}

/// The workflows, for a source that runs one (docs/modules/workflow.md,
/// triggers).
#[async_trait]
pub trait Runner: Send + Sync {
    /// Run workflow `workflow` by its `webhook` trigger of `source` with
    /// `body` as sent: the run's id.
    ///
    /// # Errors
    /// No workflow module runs; it refused (no such workflow or trigger, a
    /// body that does not read, grants not approved) or failed.
    async fn run(&self, workflow: &str, source: &str, body: Vec<u8>)
    -> Result<String, ModuleError>;
}

/// The board, memory, the channel and the workflows of a source.
pub trait Reached: Board + Incidents + Teller + Runner {}

impl<T: Board + Incidents + Teller + Runner> Reached for T {}

/// Where the sources' tasks and incidents go.
pub trait World: Send + Sync {
    /// The board and memory in `source`'s workspace, as the source.
    fn of<'a>(&'a self, source: &'a Source) -> Box<dyn Reached + 'a>;
}
