// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What the `dynamic` module needs of the world besides its documents
//! (`pcx-documents`): memory's blobs, the launcher that prepares and checks a
//! bundle, the board a person approves on, what its author reaches, the bus.

use std::collections::BTreeSet;
use std::path::PathBuf;

use async_trait::async_trait;
use pcx_module_api::{ModuleError, Scope};
use post_cortex_proto::board::{
    AddTaskRequest, DecideRequest, DecisionRecord, TaskRecord, Thread, WithdrawDecisionRequest,
};
use post_cortex_proto::dynamic::{Check, File, Grants, Language};
use post_cortex_proto::pb::Attachment;

/// Bundles as memory keeps them.
#[async_trait]
pub trait Blobs: Send + Sync {
    /// Keep the tar `bytes`.
    ///
    /// # Errors
    /// Memory refused or failed.
    async fn put(&self, bytes: Vec<u8>) -> Result<Attachment, ModuleError>;

    /// The tar of hash `sha256`.
    ///
    /// # Errors
    /// Memory failed, or has none.
    async fn get(&self, sha256: &str) -> Result<Vec<u8>, ModuleError>;
}

/// A bundle prepared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prepared {
    /// Its files, read-only.
    pub bundle: PathBuf,
    /// Its packages.
    pub environment: Option<PathBuf>,
}

/// What a locked run checks.
#[derive(Debug, Clone, Copy)]
pub struct Checking<'a> {
    /// The module's id, which its manifest must name.
    pub id: &'a str,
    /// Its language.
    pub language: Language,
    /// What it asks for, past which its manifest is refused.
    pub grants: &'a Grants,
    /// Whether it has tests.
    pub has_tests: bool,
    /// Its memory limit, in MB.
    pub memory_mb: u32,
}

/// What prepares and checks bundles.
#[async_trait]
pub trait Bundles: Send + Sync {
    /// The tar `bytes` of hash `sha256` prepared, or as it was kept.
    ///
    /// # Errors
    /// `FailedPrecondition` naming what cannot be met; it failed.
    async fn prepare(&self, bytes: Vec<u8>, sha256: &str) -> Result<Prepared, ModuleError>;

    /// `prepared` run locked, as `checking` says: what it declares, its tests.
    ///
    /// # Errors
    /// `FailedPrecondition` when it does not start or its manifest is
    /// refused, saying why.
    async fn check(
        &self,
        checking: Checking<'_>,
        prepared: &Prepared,
    ) -> Result<Check, ModuleError>;
}

/// The board a person approves on, asked in the name `asker`.
#[async_trait]
pub trait Board: Send + Sync {
    /// Add `task`, once by its key.
    ///
    /// # Errors
    /// The board refused or failed.
    async fn task(&self, asker: &str, task: AddTaskRequest) -> Result<TaskRecord, ModuleError>;

    /// Record `question`.
    ///
    /// # Errors
    /// The board refused or failed.
    async fn decide(
        &self,
        asker: &str,
        question: DecideRequest,
    ) -> Result<DecisionRecord, ModuleError>;

    /// Task `task` with its thread.
    ///
    /// # Errors
    /// The board refused or failed.
    async fn thread(&self, asker: &str, task: &str) -> Result<Thread, ModuleError>;

    /// Withdraw a decision `asker` made, as `request` says.
    ///
    /// # Errors
    /// `FailedPrecondition` when it waits no more; the board refused or failed.
    async fn withdraw(
        &self,
        asker: &str,
        request: WithdrawDecisionRequest,
    ) -> Result<DecisionRecord, ModuleError>;
}

/// The files a caller's tools reach.
#[async_trait]
pub trait Directory: Send + Sync {
    /// The bundle's files in `dir`, as the caller's `tools-fs` reads them.
    ///
    /// # Errors
    /// `FailedPrecondition` when the caller reaches no `tools-fs`;
    /// `InvalidArgument` for more files than a bundle holds; its failure.
    async fn files(&self, scope: &Scope, dir: &str) -> Result<Vec<File>, ModuleError>;
}

/// What a caller reaches.
#[async_trait]
pub trait Reach: Send + Sync {
    /// The modules `scope`'s author may call.
    ///
    /// # Errors
    /// The host failed.
    async fn modules(&self, scope: &Scope) -> Result<BTreeSet<String>, ModuleError>;
}

/// What a host hears of.
#[async_trait]
pub trait Events: Send + Sync {
    /// The approved set changed: a host loads it again.
    async fn changed(&self, id: &str, removed: bool);
}
