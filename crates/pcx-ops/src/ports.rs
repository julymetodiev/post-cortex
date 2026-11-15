// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What ops needs of the world (ARCHITECTURE section 8, operations): an
//! environment's cluster, Kubernetes or Nomad (read before acting, then
//! changed), the board (a
//! decision recorded on the task, read back, a comment), the channel and
//! the time. The module's adapters fill them; tests fill them in memory.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use pcx_module_api::ModuleError;
use post_cortex_proto::board::DecisionRecord;
use post_cortex_proto::ops::{
    ClaimPatch, DeploymentPatch, Environment, KubeThing, NomadAlloc, NomadChildren,
    NomadDeployment, NomadJob, Revert, ThingList,
};
use post_cortex_proto::team::Stakes;

/// A merge patch ops sends.
#[derive(Debug, Clone, PartialEq)]
pub enum Patch {
    /// Of a deployment (a restart) or its scale.
    Deployment(DeploymentPatch),
    /// Of a volume claim.
    Claim(ClaimPatch),
}

/// An environment's cluster, with ops' own account.
#[async_trait]
pub trait Cluster: Send + Sync {
    /// The object at `path`.
    ///
    /// # Errors
    /// `NotFound` when there is none; the cluster refused or failed.
    async fn get(&self, environment: &Environment, path: &str) -> Result<KubeThing, ModuleError>;

    /// The list at `path`.
    ///
    /// # Errors
    /// As [`Cluster::get`].
    async fn list(&self, environment: &Environment, path: &str) -> Result<ThingList, ModuleError>;

    /// Merge `patch` into the object at `path`.
    ///
    /// # Errors
    /// As [`Cluster::get`].
    async fn patch(
        &self,
        environment: &Environment,
        path: &str,
        patch: Patch,
    ) -> Result<(), ModuleError>;

    /// Delete the object at `path` (its dependents in the background).
    ///
    /// # Errors
    /// As [`Cluster::get`].
    async fn delete(&self, environment: &Environment, path: &str) -> Result<(), ModuleError>;
}

/// A Nomad environment, with ops' own token.
#[async_trait]
pub trait Nomad: Send + Sync {
    /// Job `name` as it runs now.
    ///
    /// # Errors
    /// `NotFound` when there is none; Nomad refused or failed.
    async fn job(&self, environment: &Environment, name: &str) -> Result<NomadJob, ModuleError>;

    /// The versions of job `name` Nomad keeps.
    ///
    /// # Errors
    /// As [`Nomad::job`].
    async fn versions(
        &self,
        environment: &Environment,
        name: &str,
    ) -> Result<Vec<NomadJob>, ModuleError>;

    /// The latest deployment of job `name`; none when it never had one.
    ///
    /// # Errors
    /// As [`Nomad::job`].
    async fn deployment(
        &self,
        environment: &Environment,
        name: &str,
    ) -> Result<Option<NomadDeployment>, ModuleError>;

    /// How many launches of the periodic job `name` wait and run.
    ///
    /// # Errors
    /// As [`Nomad::job`].
    async fn launches(
        &self,
        environment: &Environment,
        name: &str,
    ) -> Result<NomadChildren, ModuleError>;

    /// The allocations whose id starts with `start`, an even number of
    /// digits.
    ///
    /// # Errors
    /// Nomad refused or failed.
    async fn allocations(
        &self,
        environment: &Environment,
        start: &str,
    ) -> Result<Vec<NomadAlloc>, ModuleError>;

    /// Stop allocation `id`: Nomad places a new one of its job.
    ///
    /// # Errors
    /// As [`Nomad::job`].
    async fn stop(&self, environment: &Environment, id: &str) -> Result<(), ModuleError>;

    /// Launch the periodic job `name` once now.
    ///
    /// # Errors
    /// As [`Nomad::job`].
    async fn launch(&self, environment: &Environment, name: &str) -> Result<(), ModuleError>;

    /// Take `revert`'s job back to the version it names, if it still runs
    /// the version it was asked from.
    ///
    /// # Errors
    /// As [`Nomad::job`]; it runs another version by now.
    async fn revert(&self, environment: &Environment, revert: &Revert) -> Result<(), ModuleError>;
}

/// A decision ops records.
#[derive(Debug, Clone, PartialEq)]
pub struct Decision {
    /// The workspace of its task.
    pub workspace: String,
    /// Its task.
    pub task: String,
    /// What it decides.
    pub what: String,
    /// Why.
    pub why: String,
    /// What it puts at stake.
    pub stakes: Stakes,
}

/// The board, for ops.
#[async_trait]
pub trait Board: Send + Sync {
    /// Record `decision` as `author` (risk operations): the record, which
    /// the team's trust may have approved at once.
    ///
    /// # Errors
    /// The board refused (no such task, an author without authority) or
    /// failed.
    async fn decide(&self, author: &str, decision: Decision)
    -> Result<DecisionRecord, ModuleError>;

    /// Decision `id` of task `task` of `workspace`; none when it is gone.
    ///
    /// # Errors
    /// The board failed.
    async fn decision(
        &self,
        workspace: &str,
        task: &str,
        id: &str,
    ) -> Result<Option<DecisionRecord>, ModuleError>;

    /// Withdraw decision `id` of `workspace` as `author`, who asked it,
    /// because of `why`: it waits no more.
    ///
    /// # Errors
    /// The board refused (it waits no more, another's decision) or failed.
    async fn withdraw(
        &self,
        author: &str,
        workspace: &str,
        id: &str,
        why: &str,
    ) -> Result<(), ModuleError>;

    /// Comment `text` on task `task` of `workspace`, as ops.
    ///
    /// # Errors
    /// The board refused or failed.
    async fn comment(&self, workspace: &str, task: &str, text: &str) -> Result<(), ModuleError>;
}

/// The channel, to tell people how an action ended.
#[async_trait]
pub trait Teller: Send + Sync {
    /// Send `text` to channel destination `destination`.
    ///
    /// # Errors
    /// No channel runs, or it refused or failed.
    async fn tell(&self, destination: &str, text: &str) -> Result<(), ModuleError>;
}

/// The time.
pub trait Clock: Send + Sync {
    /// Now.
    fn now(&self) -> DateTime<Utc>;
}
