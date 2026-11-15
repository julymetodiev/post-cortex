// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A workflow written and its grants approved (docs/modules/workflow.md,
//! triggers and approval): a new workflow, or a change past the grants a
//! person approved, adds the workflow's task to the settings' board with a
//! decision of risk `code`; the decision approved (heard on the bus, or
//! read again when the module starts) approves the grants, and one a
//! person declined is kept as declined: the workflow asks no one until it
//! is written again. A run that waited on a decision when the module
//! stopped reads it again too (`answered`).

use std::collections::HashMap;

use pcx_approval::Place;
use pcx_module_api::{ModuleError, ModuleErrorKind, Scope};
use post_cortex_proto::board::{DecisionApproved, DecisionDeclined, DecisionRecord, TaskRecord};
use post_cortex_proto::module::BusEvent;
use post_cortex_proto::workflow::Workflow;
use tokio_util::sync::CancellationToken;

use crate::domain::grants;
use crate::ports::{Kept, Whose, Workflows};
use crate::service::Service;
use crate::service::engine::Engine;
use crate::service::engine::act::{ADD_TASK, DECIDE};
use crate::service::engine::answered::Answered;

/// Times a workflow is read again when another write came between.
const TRIES: u32 = 4;

impl Engine {
    /// `workflow` checked for `scope`'s author and kept in `workflows` as
    /// a new one, its grants asked for on the board.
    ///
    /// # Errors
    /// `FailedPrecondition` when the settings name no board to approve it
    /// on, or what its `agent` blocks ask is not there (a member of the
    /// workspace's teams, the agent, a model), naming what a person adds;
    /// the service's; the board refused or failed.
    pub async fn create(
        &self,
        scope: &Scope,
        workflows: &dyn Workflows,
        workflow: &Workflow,
    ) -> Result<Kept<Workflow>, ModuleError> {
        if self.approvals(true).await?.is_none() {
            return Err(ModuleError::new(
                ModuleErrorKind::FailedPrecondition,
                "a workflow's grants are approved on a board, and the Workflow module's settings name none: choose one in its setting \"Board of approvals\"",
            )
            .with_field("board"));
        }
        self.ready(workflow).await?;
        let written = grants::kept(workflow.clone(), None);
        let kept = Service::new(workflows, self.ports.tools.as_ref())
            .create(scope, &written)
            .await?;
        let id = kept.id.clone();
        match self.written(kept).await {
            Ok(asked) => Ok(asked),
            Err(refused) => {
                // All or nothing: a new workflow whose grants could not be
                // asked is not kept.
                if let Err(e) = workflows.delete(&id).await {
                    tracing::warn!(workflow = %workflow.name, "a workflow whose grants were not asked stays; the next start asks: {e}");
                }
                Err(refused)
            }
        }
    }

    /// What `workflow`'s `agent` blocks ask is there: each member they
    /// name, and the agent with a model.
    async fn ready(&self, workflow: &Workflow) -> Result<(), ModuleError> {
        let wanted = grants::wanted(workflow);
        for member in &wanted.members {
            self.ports
                .turns
                .ready(Whose::Member(member))
                .await
                .map_err(|e| e.with_field("agent.member"))?;
        }
        if grants::own_turn(workflow) {
            self.ports
                .turns
                .ready(Whose::Agent(&wanted.modules))
                .await?;
        }
        Ok(())
    }

    /// The workflow template `name` of `module` makes with `values`, kept
    /// in `workflows` as [`Engine::create`] keeps a new one.
    ///
    /// # Errors
    /// The service's (no such template, values it refuses); `create`'s.
    pub async fn install(
        &self,
        scope: &Scope,
        workflows: &dyn Workflows,
        module: &str,
        name: &str,
        values: &HashMap<String, String>,
    ) -> Result<Kept<Workflow>, ModuleError> {
        let workflow = Service::new(workflows, self.ports.tools.as_ref())
            .template(scope, module, name, values)
            .await?;
        self.create(scope, workflows, &workflow).await
    }

    /// Workflow `id` changed to `workflow` from `version` in `workflows`,
    /// for `scope`'s author: what was approved kept, grants past it asked
    /// for on the board.
    ///
    /// # Errors
    /// `NotFound` for no such version; the service's; the board refused
    /// or failed.
    pub async fn update(
        &self,
        scope: &Scope,
        workflows: &dyn Workflows,
        id: &str,
        version: u64,
        workflow: &Workflow,
    ) -> Result<Kept<Workflow>, ModuleError> {
        let previous = workflows.at(id, version).await?.ok_or_else(|| {
            ModuleError::new(
                ModuleErrorKind::NotFound,
                format!("no workflow {id} at version {version}"),
            )
        })?;
        let written = grants::kept(workflow.clone(), Some(&previous.data));
        let kept = Service::new(workflows, self.ports.tools.as_ref())
            .update(scope, id, version, &written)
            .await?;
        self.written(kept).await
    }

    /// Workflow `id` deleted from `workflows`: it starts no more, its `event`
    /// triggers read again. A run reads its version as it goes, so none may be
    /// open; the ended ones stay to be read.
    ///
    /// # Errors
    /// `FailedPrecondition` while a run of it is open; memory failed.
    pub async fn delete(&self, workflows: &dyn Workflows, id: &str) -> Result<(), ModuleError> {
        let open = self.ports.runs.open().await?;
        if open.iter().any(|run| run.data.workflow_id == id) {
            return Err(ModuleError::new(
                ModuleErrorKind::FailedPrecondition,
                "a run of this workflow is open: cancel it or wait for it to end, then delete",
            ));
        }
        let gone = workflows.get(id).await?;
        workflows.delete(id).await?;
        // Its grants asked and not answered: no one is asked any more.
        if let Some(workflow) = gone
            && let Some(asked) = &workflow.data.asked
            && asked.declined_by.is_empty()
        {
            let scope = self.scope(&workflow.data.name, 0, CancellationToken::new());
            let why = format!("Workflow {} was deleted.", workflow.data.name);
            self.withdraw(&scope, &asked.decision, why).await;
        }
        self.listen().await
    }

    /// `kept`, just written: its grants asked for when they need a
    /// decision, the `event` triggers read again.
    async fn written(&self, kept: Kept<Workflow>) -> Result<Kept<Workflow>, ModuleError> {
        let kept = if grants::asks(&kept.data) {
            self.ask(kept).await?
        } else {
            kept
        };
        self.listen().await?;
        Ok(kept)
    }

    /// The decision on `kept`'s grants recorded on its task (added once,
    /// key `workflow:<id>`), and written in it; approved at once when the
    /// team's policy approves it.
    async fn ask(&self, kept: Kept<Workflow>) -> Result<Kept<Workflow>, ModuleError> {
        let scope = self.scope(&kept.data.name, 0, CancellationToken::new());
        // A question no one answered asks for what it no longer calls.
        if let Some(asked) = &kept.data.asked
            && asked.declined_by.is_empty()
        {
            let why = format!(
                "Workflow {} asks again for what it calls now.",
                kept.data.name
            );
            self.withdraw(&scope, &asked.decision, why).await;
        }
        let board = self.approvals(true).await?.unwrap_or_default();
        let place = Place {
            workspace: &self.settings.workspace,
            board: &board,
        };
        let task: TaskRecord = self
            .board(
                &scope,
                ADD_TASK,
                &pcx_approval::task(
                    place,
                    format!("Workflow {}", kept.data.name),
                    kept.data.description.clone(),
                    format!("workflow:{}", kept.id),
                ),
            )
            .await?;
        let decided: DecisionRecord = self
            .board(
                &scope,
                DECIDE,
                &pcx_approval::question(
                    place,
                    task.id.clone(),
                    grants::question(&kept.data),
                    format!(
                        "Workflow {} runs only with the grants a person approved.",
                        kept.data.name
                    ),
                ),
            )
            .await?;
        let approved_by = pcx_approval::approved_at_once(&decided);
        let (task, decision) = (task.id, decided.id);
        self.change_workflow(&kept.id, |w| {
            let asked = grants::asked(w.clone(), &task, &decision);
            match &approved_by {
                Some(by) => grants::approve(asked, &decision, by),
                None => Some(asked),
            }
        })
        .await
        .map(|changed| changed.unwrap_or(kept))
    }

    /// Workflow `id` changed by `change` from its latest version (none:
    /// left as it is), read again when another write came between: the
    /// version written.
    async fn change_workflow(
        &self,
        id: &str,
        change: impl Fn(&Workflow) -> Option<Workflow>,
    ) -> Result<Option<Kept<Workflow>>, ModuleError> {
        let mut tries = 0;
        loop {
            let Some(latest) = self.ports.workflows.get(id).await? else {
                return Ok(None);
            };
            let Some(changed) = change(&latest.data) else {
                return Ok(None);
            };
            match self
                .ports
                .workflows
                .change(id, latest.version, &changed)
                .await
            {
                Err(e) if e.kind == ModuleErrorKind::Conflict && tries < TRIES => tries += 1,
                written => return written.map(Some),
            }
        }
    }

    /// A `board.decision_approved` or `board.decision_declined` heard: the
    /// workflow that asked it has its grants approved, or keeps that a
    /// person declined them.
    ///
    /// # Errors
    /// Memory failed.
    pub async fn decided(&self, event: &BusEvent) -> Result<(), ModuleError> {
        if let Some(approved) = pcx_approval::heard(event) {
            return self.approve(&approved).await;
        }
        match pcx_approval::declined(event) {
            Some(declined) => self.decline(&declined).await,
            None => Ok(()),
        }
    }

    async fn decline(&self, declined: &DecisionDeclined) -> Result<(), ModuleError> {
        let asked = self.ports.workflows.all().await?.into_iter().find(|w| {
            w.data
                .asked
                .as_ref()
                .is_some_and(|a| a.decision == declined.decision)
        });
        if let Some(workflow) = asked {
            let kept = self
                .change_workflow(&workflow.id, |w| {
                    grants::decline(w.clone(), &declined.decision, &declined.declined_by)
                })
                .await?;
            if kept.is_some() {
                tracing::info!(workflow = %workflow.data.name, by = %declined.declined_by, "a workflow's grants were declined");
            }
        }
        Ok(())
    }

    async fn approve(&self, approved: &DecisionApproved) -> Result<(), ModuleError> {
        let asked = self.ports.workflows.all().await?.into_iter().find(|w| {
            w.data
                .asked
                .as_ref()
                .is_some_and(|a| a.decision == approved.decision)
        });
        if let Some(workflow) = asked {
            self.change_workflow(&workflow.id, |w| {
                grants::approve(w.clone(), &approved.decision, &approved.approved_by)
            })
            .await?;
            tracing::info!(workflow = %workflow.data.name, by = %approved.approved_by, "a workflow's grants were approved");
        }
        Ok(())
    }

    /// After the module starts: each workflow's decision on its grants
    /// read again from its task, an approval made meanwhile approving it.
    ///
    /// # Errors
    /// Memory failed (a board that does not answer is logged: the event
    /// may still come).
    pub async fn recheck(&self) -> Result<(), ModuleError> {
        for workflow in self.ports.workflows.all().await? {
            let workflow = self.counted(workflow).await?;
            if let Some(asked) = &workflow.data.asked
                && asked.declined_by.is_empty()
                && grants::asks(&workflow.data)
            {
                // Asked for other grants than it needs now: asked again.
                let name = workflow.data.name.clone();
                if let Err(e) = self.ask(workflow).await {
                    tracing::warn!(workflow = %name, "its grants could not be asked: {e}");
                }
                continue;
            }
            let Some(asked) = workflow.data.asked.clone() else {
                // One whose grants were never asked (the board refused):
                // asked now.
                if grants::asks(&workflow.data) && self.approvals(false).await?.is_some() {
                    let name = workflow.data.name.clone();
                    if let Err(e) = self.ask(workflow).await {
                        tracing::warn!(workflow = %name, "its grants could not be asked: {e}");
                    }
                }
                continue;
            };
            if !asked.declined_by.is_empty() {
                continue;
            }
            let scope = self.scope(&workflow.data.name, 0, CancellationToken::new());
            match self.answered_on(&scope, &asked.task, &asked.decision).await {
                Ok(Some(Answered::Approved(approved))) => self.approve(&approved).await?,
                Ok(Some(Answered::Declined(declined))) => self.decline(&declined).await?,
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(workflow = %workflow.data.name, "its decision could not be read again: {e}");
                }
            }
        }
        Ok(())
    }

    /// `workflow` with the tools of effect `external` its graph calls as a
    /// run of it reaches them now, written when they are not those it keeps
    /// (one written before tools were asked for keeps none); as it is when
    /// it does not check now (a module not running), which is logged.
    async fn counted(&self, workflow: Kept<Workflow>) -> Result<Kept<Workflow>, ModuleError> {
        let scope = self.scope(&workflow.data.name, 0, CancellationToken::new());
        let found = match self.service().checked(&scope, &workflow.data).await {
            Ok((_, checked)) => checked.found,
            Err(e) => {
                tracing::warn!(workflow = %workflow.data.name, "what it calls could not be read again: {e}");
                return Ok(workflow);
            }
        };
        let tools = grants::wanted(&grants::calling(workflow.data.clone(), &found)).tools;
        if tools == grants::wanted(&workflow.data).tools {
            return Ok(workflow);
        }
        let changed = self
            .change_workflow(&workflow.id, |w| {
                let mut w = w.clone();
                w.grants.get_or_insert_default().tools.clone_from(&tools);
                Some(w)
            })
            .await?;
        Ok(changed.unwrap_or(workflow))
    }
}
