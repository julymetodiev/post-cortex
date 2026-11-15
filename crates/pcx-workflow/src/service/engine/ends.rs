// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A run's document changed as it goes (its status, what it spent, its
//! task) by whichever writer comes, and ended: its event emitted, a third
//! failure in a row told, its parent woken.

use chrono::{DateTime, Utc};
use pcx_module_api::{ModuleError, ModuleErrorKind};
use post_cortex_proto::workflow::{Failure, Run, RunEnded, RunStatus};
use prost::{Message, Name};
use tokio_util::sync::CancellationToken;

use crate::domain::{self, failure, kinds, open};
use crate::ports::Kept;
use crate::service::engine::{Engine, FAILING, Written, events, now_ts};

impl Engine {
    pub async fn latest(&self, id: &str) -> Result<Kept<Run>, ModuleError> {
        self.ports
            .runs
            .get(id)
            .await?
            .ok_or_else(|| ModuleError::new(ModuleErrorKind::NotFound, format!("no run {id}")))
    }

    /// Run `id` changed by `change` from its latest version, read again
    /// and changed again when another writer (the driver, a step's task)
    /// wrote it meanwhile.
    ///
    /// # Errors
    /// `NotFound`; `Conflict` after a few tries; memory failed.
    pub async fn update_run(
        &self,
        id: &str,
        change: impl Fn(&mut Run),
    ) -> Result<Kept<Run>, ModuleError> {
        let mut tries = 0;
        loop {
            let run = self.latest(id).await?;
            let mut changed = run.data.clone();
            change(&mut changed);
            if changed == run.data {
                return Ok(run);
            }
            match self.ports.runs.change(id, run.version, &changed).await {
                Err(e) if e.kind == ModuleErrorKind::Conflict && tries < 4 => tries += 1,
                Ok(written) => {
                    self.wrote(Written::Run(Box::new(written.clone())));
                    return Ok(written);
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// Run `run`'s status written when it changed.
    pub async fn mark(
        &self,
        run: &Kept<Run>,
        status: RunStatus,
        until: Option<DateTime<Utc>>,
    ) -> Result<(), ModuleError> {
        let until = until.map(domain::timestamp);
        self.update_run(&run.id, |r| {
            if open(r) {
                r.status = status.into();
                r.waits_until = until;
            }
        })
        .await
        .map(|_| ())
    }

    /// Run `run` failed on a budget.
    pub async fn over(&self, run: &Kept<Run>, why: &str) -> Result<(), ModuleError> {
        self.end(
            run,
            RunStatus::Failed,
            None,
            Some(failure(kinds::BUDGET, why)),
        )
        .await
        .map(|_| ())
    }

    /// Run `run` ended with `status`: the steps it leaves waiting closed
    /// (the decisions its approvals waited on withdrawn), written, its
    /// event emitted, its parent woken.
    pub async fn end(
        &self,
        run: &Kept<Run>,
        status: RunStatus,
        output: Option<post_cortex_proto::workflow::Value>,
        failed: Option<Failure>,
    ) -> Result<Kept<Run>, ModuleError> {
        // Before the run reads as ended: a watcher of the run follows it
        // only until then.
        if !self.dry() {
            self.settle(run, failed.as_ref()).await;
        }
        let ended_at = Some(now_ts(self.ports.clock.as_ref()));
        let kept = self
            .update_run(&run.id, |r| {
                r.status = status.into();
                r.ended_at = ended_at;
                r.waits_until = None;
                r.output = output.clone();
                r.failure = failed.clone();
            })
            .await?;
        // In the host's log too, so a failure is seen where no one reads
        // the runs; its kind only: its message may carry what a stranger
        // wrote, and the log is read by agents (docs/modules/workflow.md).
        if status == RunStatus::Failed && !self.dry() {
            let kind = failed.as_ref().map_or("", |f| f.kind.as_str());
            tracing::warn!(workflow = %kept.data.workflow, run = %kept.id, kind, "a workflow's run failed");
        }
        let name = if status == RunStatus::Succeeded {
            events::SUCCEEDED
        } else {
            events::FAILED
        };
        self.emit(
            &kept.data,
            name,
            &RunEnded {
                run: kept.id.clone(),
                workflow: kept.data.workflow.clone(),
                status: status.into(),
                failure: failed.clone(),
            },
        )
        .await;
        if status == RunStatus::Failed && self.failing(&kept.data.workflow).await? {
            self.emit(
                &kept.data,
                events::FAILING,
                &RunEnded {
                    run: kept.id.clone(),
                    workflow: kept.data.workflow.clone(),
                    status: status.into(),
                    failure: failed,
                },
            )
            .await;
        }
        if let Some(parent) = self.live.lock().get(&self.live_key(&kept.data.parent)) {
            parent.wake.notify_one();
        }
        Ok(kept)
    }

    /// Whether the [`FAILING`] runs of `workflow` that ended last all
    /// failed.
    pub async fn failing(&self, workflow: &str) -> Result<bool, ModuleError> {
        let ended = self.ports.runs.ended(workflow, FAILING).await?;
        Ok(ended.len() == FAILING
            && ended
                .iter()
                .all(|r| r.data.status == i32::from(RunStatus::Failed)))
    }

    /// Emit `name` with `message`; a failure is logged, the run stands. A
    /// dry run emits nothing.
    pub async fn emit<T: Message + Name>(&self, run: &Run, name: &str, message: &T) {
        if self.dry() {
            return;
        }
        let scope = self.scope(&run.workflow, run.call_depth, CancellationToken::new());
        let payload = pbjson_types::Any {
            type_url: T::type_url(),
            value: message.encode_to_vec().into(),
        };
        if let Err(e) = self.ports.events.publish(&scope, name, payload).await {
            tracing::warn!(workflow = %run.workflow, "{name} was not emitted: {e}");
        }
    }
}
