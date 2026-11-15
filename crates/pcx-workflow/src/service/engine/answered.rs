// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A decision read again from the board (docs/modules/workflow.md,
//! triggers and approval): how it was answered while no one heard, for a
//! workflow's grants when the module starts and for a run's approvals that
//! waited on one when the module stopped.

use pcx_approval::{DECISION_APPROVED, DECISION_DECLINED};
use pcx_module_api::{ModuleError, Scope};
use post_cortex_proto::board::{
    Decision, DecisionApproved, DecisionDeclined, GetTaskRequest, Thread,
};
use post_cortex_proto::module::BusEvent;
use post_cortex_proto::workflow::{Step, StepStatus, Workflow, block::Kind};
use prost::{Message, Name};

use crate::domain;
use crate::service::engine::Engine;
use crate::service::engine::act::{self, GET_TASK};

impl Engine {
    /// Decision `decision` on task `task`, read from the board in `scope`:
    /// as the board's event says it, when a person or the policy answered
    /// it.
    ///
    /// # Errors
    /// The board refused or failed.
    pub async fn answered_on(
        &self,
        scope: &Scope,
        task: &str,
        decision: &str,
    ) -> Result<Option<Answered>, ModuleError> {
        let thread: Thread = self
            .board(
                scope,
                GET_TASK,
                &GetTaskRequest {
                    workspace_id: self.settings.workspace.clone(),
                    task: task.to_string(),
                },
            )
            .await?;
        Ok(pcx_approval::approved_in(&thread, task, decision)
            .map(Answered::Approved)
            .or_else(|| pcx_approval::declined_in(&thread, task, decision).map(Answered::Declined)))
    }

    /// The approvals among `steps` of `workflow` that wait on a decision
    /// approved or declined while no one heard: each as the event the board emitted,
    /// for the run's driver to read. A board that does not answer is
    /// logged: the event may still come.
    pub async fn missed(
        &self,
        scope: &Scope,
        workflow: &Workflow,
        steps: &[Step],
    ) -> Vec<BusEvent> {
        let mut missed = Vec::new();
        for step in waiting_approvals(workflow, steps) {
            let decision = match kept_decision(step, self.own.messages()) {
                Ok(decision) => decision,
                Err(why) => {
                    tracing::warn!(run = %step.run, block = %step.block, "an approval's step keeps no decision: {why}");
                    continue;
                }
            };
            let event = |name: &str, type_url: String, value: Vec<u8>| BusEvent {
                name: name.to_string(),
                payload: Some(pbjson_types::Any {
                    type_url,
                    value: value.into(),
                }),
                ..Default::default()
            };
            match self
                .answered_on(scope, &decision.task, &step.waits_for)
                .await
            {
                Ok(Some(Answered::Approved(approved))) => missed.push(event(
                    DECISION_APPROVED,
                    DecisionApproved::type_url(),
                    approved.encode_to_vec(),
                )),
                Ok(Some(Answered::Declined(declined))) => missed.push(event(
                    DECISION_DECLINED,
                    DecisionDeclined::type_url(),
                    declined.encode_to_vec(),
                )),
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(run = %step.run, "an approval's decision could not be read again: {e}");
                }
            }
        }
        missed
    }
}

/// How a decision read again from the board was answered.
pub enum Answered {
    /// Approved, by a person or the policy.
    Approved(DecisionApproved),
    /// Declined by a person.
    Declined(DecisionDeclined),
}

/// The steps among `steps` of `workflow` whose approval waits on a
/// decision (`waits_for`).
fn waiting_approvals<'a>(
    workflow: &'a Workflow,
    steps: &'a [Step],
) -> impl Iterator<Item = &'a Step> + 'a {
    steps.iter().filter(|step| {
        step.status == i32::from(StepStatus::Waiting) && waits_on_decision(workflow, step)
    })
}

/// Whether `step` of `workflow` is an approval that waits on a decision
/// (`waits_for`).
pub fn waits_on_decision(workflow: &Workflow, step: &Step) -> bool {
    !step.waits_for.is_empty()
        && act::block(workflow.graph.as_ref(), &step.block)
            .is_some_and(|b| matches!(b.kind, Some(Kind::Approval(_))))
}

/// The decision an approval's waiting `step` keeps as its input.
fn kept_decision(
    step: &Step,
    messages: &prost_reflect::DescriptorPool,
) -> Result<Decision, String> {
    let input = step.input.as_ref().ok_or("it has no input")?;
    match domain::read_value(input, messages)? {
        pcx_expr::Value::Message(message) => message
            .transcode_to::<Decision>()
            .map_err(|e| format!("its input is no decision: {e}")),
        other => Err(format!("its input is a {}, no decision", other.ty())),
    }
}
