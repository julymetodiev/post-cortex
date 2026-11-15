// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What a run leaves waiting when it ends (docs/modules/workflow.md, the
//! engine's reach, withdrawn): its waiting steps closed, the decisions its
//! approvals waited on withdrawn, so no person is asked what no run waits
//! for; at start, the same for the runs that ended last of each workflow
//! (one that ended before this, or whose board did not answer).

use pcx_approval::WITHDRAW;
use pcx_module_api::{ModuleError, ModuleErrorKind, Scope};
use post_cortex_proto::board::DecisionRecord;
use post_cortex_proto::workflow::{Failure, Run, StepStatus};
use tokio_util::sync::CancellationToken;

use crate::domain::{failure, kinds};
use crate::ports::Kept;
use crate::service::engine::answered::waits_on_decision;
use crate::service::engine::{Engine, Written, now_ts};

/// The runs of each workflow that ended last, settled again at start.
const SWEPT: usize = 50;

impl Engine {
    /// After the module starts: the runs of each workflow that ended last
    /// settled, so a decision left waiting by a run that ended is
    /// withdrawn.
    ///
    /// # Errors
    /// Memory failed.
    pub async fn sweep(&self) -> Result<(), ModuleError> {
        for workflow in self.ports.workflows.all().await? {
            for run in self.ports.runs.ended(&workflow.data.name, SWEPT).await? {
                self.settle(&run, run.data.failure.as_ref()).await;
            }
        }
        Ok(())
    }

    /// The steps run `run` leaves waiting as it ends (because of `failed`,
    /// none when it succeeded) closed, an approval's decision withdrawn
    /// first. A board that does not answer leaves its step waiting, for
    /// the next start; a failure is logged, the run's end stands.
    pub async fn settle(&self, run: &Kept<Run>, failed: Option<&Failure>) {
        let steps = match self.ports.runs.steps(&run.id).await {
            Ok(steps) => steps,
            Err(e) => {
                tracing::warn!(run = %run.id, "its waiting steps were not closed: {e}");
                return;
            }
        };
        let waiting: Vec<_> = steps
            .into_iter()
            .filter(|s| s.data.status == i32::from(StepStatus::Waiting))
            .collect();
        if waiting.is_empty() {
            return;
        }
        let workflow = match self
            .ports
            .workflows
            .at(&run.data.workflow_id, run.data.version)
            .await
        {
            Ok(Some(workflow)) => workflow.data,
            Ok(None) => return,
            Err(e) => {
                tracing::warn!(run = %run.id, "its waiting steps were not closed: {e}");
                return;
            }
        };
        let scope = self.scope(
            &workflow.name,
            run.data.call_depth,
            CancellationToken::new(),
        );
        let why = match failed {
            Some(failed) => format!(
                "Run {} of {} ended while it waited on this decision: {}",
                run.id, workflow.name, failed.message
            ),
            None => format!(
                "Run {} of {} ended while it waited on this decision.",
                run.id, workflow.name
            ),
        };
        for step in waiting {
            if waits_on_decision(&workflow, &step.data)
                && !self
                    .withdraw(&scope, &step.data.waits_for, why.clone())
                    .await
            {
                continue;
            }
            let mut closed = step.data.clone();
            closed.status = StepStatus::Failed.into();
            closed.ended_at = Some(now_ts(self.ports.clock.as_ref()));
            closed.failure = Some(failure(kinds::CANCELLED, "the run ended while it waited"));
            match self
                .ports
                .runs
                .change_step(&step.id, step.version, &closed)
                .await
            {
                Ok(written) => self.wrote(Written::Step(Box::new(written.data))),
                Err(e) => {
                    tracing::warn!(run = %run.id, block = %step.data.block, "a waiting step was not closed: {e}");
                }
            }
        }
    }

    /// Decision `decision` withdrawn in `scope` (its author's) because of
    /// `why`: whether it waits no more (withdrawn now, or approved or
    /// withdrawn before). A board that does not answer is logged.
    pub async fn withdraw(&self, scope: &Scope, decision: &str, why: String) -> bool {
        let request = pcx_approval::withdrawal(&self.settings.workspace, decision.to_string(), why);
        let withdrawn: Result<DecisionRecord, ModuleError> =
            self.board(scope, WITHDRAW, &request).await;
        match withdrawn {
            Ok(_) => true,
            // It waits for no approval: approved or withdrawn already.
            Err(e) if e.kind == ModuleErrorKind::FailedPrecondition => true,
            Err(e) => {
                tracing::warn!(decision = %decision, "a decision no one waits on was not withdrawn: {e}");
                false
            }
        }
    }
}
