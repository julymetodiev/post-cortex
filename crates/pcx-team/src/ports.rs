// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What a member's work needs of the world (ARCHITECTURE section 8) besides
//! the team's documents and new sessions (`pcx-documents`) and the agent's
//! turns (`pcx_module_api::agent::Turns`): the board's changes, tasks and
//! threads and the skills' instructions. The module's adapters fill them
//! over the host handle; tests fill them in memory.

use async_trait::async_trait;
use pcx_module_api::ModuleError;

use crate::wake::{Change, Thread};

/// A task's title and column.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Heading {
    /// Its title.
    pub title: String,
    /// Its column.
    pub column: String,
}

/// The board, for the team.
#[async_trait]
pub trait Board: Send + Sync {
    /// The change version `version` of the board's document `id` (of
    /// `kind`, `board/task` or `board/comment`) made; none when it is gone
    /// or wakes no one (another kind, a deleted one).
    ///
    /// # Errors
    /// Memory failed.
    async fn change(
        &self,
        kind: &str,
        id: &str,
        version: u64,
    ) -> Result<Option<Change>, ModuleError>;

    /// Task `task`'s title and column; none when it is gone.
    ///
    /// # Errors
    /// Memory failed.
    async fn heading(&self, task: &str) -> Result<Option<Heading>, ModuleError>;

    /// The thread of task `task`.
    ///
    /// # Errors
    /// No such task; the board failed.
    async fn thread(&self, task: &str) -> Result<Thread, ModuleError>;

    /// The board's tool [`Board::answer`] comments with: a member's turn on
    /// a task is not given it, as its answer is its comment.
    fn answer_tool(&self) -> &str;

    /// Comment `text` on task `task`, answering `answers`, as the call's
    /// author.
    ///
    /// # Errors
    /// The board refused or failed.
    async fn answer(
        &self,
        task: &str,
        text: &str,
        answers: Option<&str>,
    ) -> Result<(), ModuleError>;
}

/// A task's labels and the version they were read at, and blocking it
/// (W32): what the team needs of the board when a member's work failed.
#[async_trait]
pub trait Labels: Send + Sync {
    /// Task `task`'s version and labels.
    ///
    /// # Errors
    /// No such task; the board failed.
    async fn labels(&self, task: &str) -> Result<(u64, Vec<String>), ModuleError>;

    /// Whether task `task` is finished, given up or gone: no work goes on it.
    ///
    /// # Errors
    /// The board failed.
    async fn closed(&self, task: &str) -> Result<bool, ModuleError>;

    /// Task `task`'s labels set to `labels` from its version `version`.
    ///
    /// # Errors
    /// It changed since (`Conflict`); the board refused or failed.
    async fn relabel(
        &self,
        task: &str,
        version: u64,
        labels: Vec<String>,
    ) -> Result<(), ModuleError>;
}

/// Where a member works on a task (W32): the repository its board names,
/// a worktree of it, and the branch the task links.
#[async_trait]
pub trait Workplace: Send + Sync {
    /// The repository of task `task`'s board; none: it names none.
    ///
    /// # Errors
    /// No such task or board; the board failed.
    async fn repository(&self, task: &str) -> Result<Option<String>, ModuleError>;

    /// A worktree of `repository` at `path` (inside it) on `branch`: its
    /// absolute directory.
    ///
    /// # Errors
    /// `tools-git` refused or failed.
    async fn worktree(
        &self,
        repository: &str,
        path: &str,
        branch: &str,
    ) -> Result<String, ModuleError>;

    /// Task `task` links `branch` (its commits show on it).
    ///
    /// # Errors
    /// The board refused or failed.
    async fn link(&self, task: &str, branch: &str) -> Result<(), ModuleError>;
}

/// The skills' library.
#[async_trait]
pub trait Library: Send + Sync {
    /// Skill `name`'s instructions; none when the library has no such
    /// skill.
    ///
    /// # Errors
    /// The library failed.
    async fn skill(&self, name: &str) -> Result<Option<String>, ModuleError>;
}
