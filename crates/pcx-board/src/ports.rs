// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What the board service needs of the world (ARCHITECTURE section 8,
//! layers) besides its documents (`pcx-documents`): the board's session
//! and its decision records, and a branch's commits. The module's adapters
//! fill them with memory's and `tools-git`'s tools over the host handle;
//! tests fill them in memory.

use async_trait::async_trait;
use pcx_module_api::ModuleError;

/// A decision as memory records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionRecord {
    /// What was decided.
    pub what: String,
    /// Why.
    pub why: String,
    /// The task's title.
    pub task: String,
    /// The board's name.
    pub board: String,
    /// Who decided.
    pub decided_by: String,
}

/// The board's sessions and records in memory.
#[async_trait]
pub trait Records: Send + Sync {
    /// A new session named `name` in the workspace; its id.
    ///
    /// # Errors
    /// Memory failed.
    async fn create_session(&self, name: &str) -> Result<String, ModuleError>;

    /// Record `decision` in `session` as a `decision_made` record.
    ///
    /// # Errors
    /// Memory refused or failed.
    async fn record_decision(
        &self,
        session: &str,
        decision: &DecisionRecord,
    ) -> Result<(), ModuleError>;
}

/// The commits of branches.
#[async_trait]
pub trait Commits: Send + Sync {
    /// The ids of the commits on `branch` of `repo` that its base lacks,
    /// newest first.
    ///
    /// # Errors
    /// `tools-git` refused (no such branch, a repository outside its
    /// roots) or failed.
    async fn branch_commits(&self, repo: &str, branch: &str) -> Result<Vec<String>, ModuleError>;
}

/// What the board tells the organisation's bus (docs/modules/board.md,
/// events), after the write it is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoardEvent {
    /// A task was added (`board.task_created`).
    TaskCreated {
        /// The task.
        task: String,
        /// Its board.
        board: String,
        /// Its title.
        title: String,
        /// Its column.
        column: String,
    },
    /// A task changed column (`board.task_moved`).
    TaskMoved {
        /// The task.
        task: String,
        /// Its board.
        board: String,
        /// The column it left.
        from: String,
        /// The column it is in.
        to: String,
    },
    /// A decision was taken (`board.decision_made`).
    DecisionMade {
        /// The decision.
        decision: String,
        /// Its task.
        task: String,
        /// Whether it waits for an approval.
        pending: bool,
    },
    /// A decision was approved (`board.decision_approved`).
    DecisionApproved {
        /// The decision.
        decision: String,
        /// Its task.
        task: String,
        /// Who approved it.
        approved_by: String,
    },
    /// A person declined a decision (`board.decision_declined`).
    DecisionDeclined {
        /// The decision.
        decision: String,
        /// Its task.
        task: String,
        /// The person who declined it.
        declined_by: String,
    },
}

/// The board's events on the organisation's bus.
#[async_trait]
pub trait Events: Send + Sync {
    /// Emit `event`. The write it is about stands whatever happens: a
    /// failure to keep the event is the adapter's to log.
    async fn emitted(&self, event: BoardEvent);
}
