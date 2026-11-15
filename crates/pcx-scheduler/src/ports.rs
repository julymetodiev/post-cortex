// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What the scheduler needs of the world (ARCHITECTURE section 8,
//! operations) besides its documents (`pcx-documents`): the time, a
//! script run under the sandbox, a task added to a board and a workflow's
//! run started. The module's adapters fill them over its host handle;
//! tests fill them in memory.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use pcx_module_api::ModuleError;
use post_cortex_proto::board::AddTaskRequest;
use post_cortex_proto::scheduler::{Script, WorkflowAction};

use crate::domain::Ended;

/// The time.
pub trait Clock: Send + Sync {
    /// Now.
    fn now(&self) -> DateTime<Utc>;
}

/// What a job does, done.
#[async_trait]
pub trait World: Send + Sync {
    /// Run `script` under the sandbox until it ends: how, and its output
    /// (stdout then stderr).
    ///
    /// # Errors
    /// The sandbox refused it (no `spawn` grant) or could not start it.
    async fn script(&self, script: &Script) -> Result<(Ended, String), ModuleError>;

    /// Add `task` to its board: the task's id (its key's task when the
    /// board has it).
    ///
    /// # Errors
    /// The board refused or failed.
    async fn add_task(&self, task: AddTaskRequest) -> Result<String, ModuleError>;

    /// Start a run of `action`'s workflow by its `schedule` trigger: the
    /// run's id.
    ///
    /// # Errors
    /// No workflow module runs; it refused (no such workflow or trigger, an
    /// input that does not read, grants not approved) or failed.
    async fn workflow(&self, action: &WorkflowAction) -> Result<String, ModuleError>;
}
