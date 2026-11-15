// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The scheduler at work: every 15 seconds the jobs due, of every
//! workspace, are run in the background, one run of a job at a time (a run due while its job runs is
//! recorded as skipped), until the module stops. The first look is one
//! interval after the start (docs/modules/scheduler.md): the host makes its
//! running set callable once every module has started.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use pcx_documents::{HostDocuments, Item};
use pcx_module_api::grpc::GrpcChannel;
use pcx_module_api::{ModuleError, ModuleErrorKind, ModuleHost, Scope};
use post_cortex_proto::scheduler::{Job, Run, RunRecord};
use tokio_util::sync::CancellationToken;

use crate::adapters::{HostWorld, SystemClock};
use crate::domain::MODULE_ID;
use crate::service::Service;

/// How often the scheduler looks for jobs due.
const TICK: Duration = Duration::from_secs(15);

/// The deadline of a run's calls to memory and the board: a script's
/// longest run (an hour) and a minute.
const RUN_DEADLINE: Duration = Duration::from_secs(3_660);

/// The scheduler's work, from its start until it stops.
pub struct Runner {
    host: Arc<dyn ModuleHost>,
    channel: GrpcChannel,
    workspace: String,
    path: Option<String>,
    /// The jobs whose run runs now, by id.
    running: Mutex<HashSet<String>>,
    stop: CancellationToken,
}

impl Runner {
    /// The work over `host` and `channel`, a call that names no workspace
    /// working in `workspace`, its scripts' PATH `path`.
    pub fn new(
        host: Arc<dyn ModuleHost>,
        channel: GrpcChannel,
        workspace: String,
        path: Option<String>,
    ) -> Arc<Self> {
        Arc::new(Self {
            host,
            channel,
            workspace,
            path,
            running: Mutex::default(),
            stop: CancellationToken::new(),
        })
    }

    /// The workspace a call in `scope` works in, whose jobs it reads and
    /// changes: the one it is asked from, else the one the settings name
    /// (docs/modules/scheduler.md, its workspaces).
    ///
    /// # Errors
    /// `Denied` for a workspace outside the call's reach.
    pub fn asked_from(&self, scope: &Scope) -> Result<String, ModuleError> {
        let workspace = scope.workspace_id.as_deref().unwrap_or(&self.workspace);
        if !scope.reaches(workspace) {
            return Err(ModuleError::new(
                ModuleErrorKind::Denied,
                format!("workspace {workspace} is outside the workspaces you can use"),
            ));
        }
        Ok(workspace.to_string())
    }

    /// The module's channel to memory and the board.
    pub fn channel(&self) -> GrpcChannel {
        self.channel.clone()
    }

    /// The scope of the scheduler's own calls in `workspace`.
    fn scope(&self, workspace: &str) -> Scope {
        Scope::own(MODULE_ID, workspace, RUN_DEADLINE, self.stop.child_token())
    }

    /// Mark job `id` running; false when it runs already.
    fn claim(&self, id: &str) -> bool {
        self.running.lock().insert(id.to_string())
    }

    /// Look for jobs due every 15 seconds until the module stops. The
    /// first look is one interval after the start, not at once: the host
    /// makes its running set callable once every module has started, and
    /// the scheduler starts before that set is swapped in.
    pub fn start(self: &Arc<Self>) {
        let runner = Arc::clone(self);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval_at(tokio::time::Instant::now() + TICK, TICK);
            loop {
                tokio::select! {
                    () = runner.stop.cancelled() => break,
                    _ = tick.tick() => {
                        if let Err(e) = runner.tick().await {
                            tracing::warn!(error = %e, "the scheduler could not look for jobs due");
                        }
                    }
                }
            }
        });
    }

    /// Stop looking, and end the scripts that run.
    pub fn stop(&self) {
        self.stop.cancel();
    }

    /// How the job named `name` stands, in whichever workspace has it. The
    /// scheduler reads it in its own name: the answer says whether a job of
    /// that name runs, and nothing else of it.
    ///
    /// # Errors
    /// `NotFound` when no workspace has a job of the name; memory failed.
    pub async fn health(&self, name: &str) -> Result<crate::domain::Health, ModuleError> {
        let every = self.scope(&self.workspace);
        for workspace in pcx_documents::every_workspace(&self.channel, &every).await? {
            let scope = self.scope(&workspace);
            let docs = HostDocuments::new(self.channel.clone(), &scope, &workspace);
            if let Some(health) = Service::new(&docs, &SystemClock).health(name).await? {
                return Ok(health);
            }
        }
        Err(ModuleError::new(
            ModuleErrorKind::NotFound,
            format!("no job is named {name}"),
        ))
    }

    /// Look in every workspace for the jobs due now. A workspace whose jobs
    /// cannot be read is said and does not hold the others.
    async fn tick(self: &Arc<Self>) -> Result<(), ModuleError> {
        let every = self.scope(&self.workspace);
        for workspace in pcx_documents::every_workspace(&self.channel, &every).await? {
            if let Err(e) = self.look(&workspace).await {
                tracing::warn!(%workspace, error = %e, "the scheduler could not look for a workspace's jobs due");
            }
        }
        Ok(())
    }

    /// Run the jobs of `workspace` due now, or record them skipped while
    /// they run.
    async fn look(self: &Arc<Self>, workspace: &str) -> Result<(), ModuleError> {
        let scope = self.scope(workspace);
        let docs = HostDocuments::new(self.channel.clone(), &scope, workspace);
        let service = Service::new(&docs, &SystemClock);
        for (job, due) in service.due().await? {
            if !self.claim(&job.document.id) {
                service.skip(&job, due).await?;
                continue;
            }
            match service.start(&job, Some(due)).await {
                Ok(run) => self.work(workspace.to_string(), job, run),
                Err(e) => {
                    self.running.lock().remove(&job.document.id);
                    return Err(e);
                }
            }
        }
        Ok(())
    }

    /// Run job `id` of `workspace` now, once: the run as it starts.
    ///
    /// # Errors
    /// `FailedPrecondition` while it runs; no such job; memory failed.
    pub async fn run_now(
        self: &Arc<Self>,
        workspace: &str,
        id: &str,
    ) -> Result<RunRecord, ModuleError> {
        let scope = self.scope(workspace);
        let docs = HostDocuments::new(self.channel.clone(), &scope, workspace);
        let service = Service::new(&docs, &SystemClock);
        let job = service.job(id).await?;
        if !self.claim(id) {
            return Err(ModuleError::new(
                ModuleErrorKind::FailedPrecondition,
                format!("job {} runs now; run it again when it ends", job.data.name),
            ));
        }
        let run = match service.start(&job, None).await {
            Ok(run) => run,
            Err(e) => {
                self.running.lock().remove(id);
                return Err(e);
            }
        };
        let started = RunRecord {
            id: run.document.id.clone(),
            run: Some(run.data.clone()),
        };
        self.work(workspace.to_string(), job, run);
        Ok(started)
    }

    /// Do `job`'s `run` in the background, in the job's `workspace`, then
    /// free the job.
    fn work(self: &Arc<Self>, workspace: String, job: Item<Job>, run: Item<Run>) {
        let runner = Arc::clone(self);
        tokio::spawn(async move {
            let scope = runner.scope(&workspace);
            let docs = HostDocuments::new(runner.channel.clone(), &scope, &workspace);
            let world = HostWorld {
                host: Arc::clone(&runner.host),
                channel: runner.channel.clone(),
                scope: scope.clone(),
                path: runner.path.clone(),
                stop: runner.stop.clone(),
            };
            let done = Service::new(&docs, &SystemClock)
                .work(&world, &job, run)
                .await;
            if let Err(e) = done {
                tracing::warn!(job = %job.data.name, error = %e, "a run's end was not written");
            }
            runner.running.lock().remove(&job.document.id);
        });
    }
}

#[cfg(test)]
mod tests;
