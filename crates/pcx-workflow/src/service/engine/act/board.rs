// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! An approval's decision on the board (docs/modules/workflow.md, the
//! engine's reach): recorded on the run's task (added once to the
//! settings' board, key `run:<id>`) or the one the block names, and read
//! approved or declined from the board's events.

use pcx_expr::Value as Expr;
use pcx_module_api::{ModuleError, ModuleErrorKind, Scope};
use post_cortex_proto::board::{AddTaskRequest, DecideRequest, DecisionRecord, TaskRecord};
use post_cortex_proto::module::BusEvent;
use post_cortex_proto::team::Stakes;
use post_cortex_proto::workflow::Failure;
use prost::Message;

use crate::domain::{as_message, failure};
use crate::service::engine::Engine;
use crate::service::engine::act::Actor;

pub use pcx_approval::{ADD_TASK, DECIDE, GET_TASK};

impl Actor {
    /// The decision `question` recorded on `task` (none: the run's, added
    /// to the settings' board once); only a person approves it when
    /// `person`.
    pub async fn decide(
        &self,
        question: &str,
        risk: i32,
        person: bool,
        task: Option<&str>,
    ) -> Result<DecisionRecord, Failure> {
        let task = match task {
            Some(task) => task.to_string(),
            None => self.task().await?,
        };
        let request = DecideRequest {
            workspace_id: self.engine.settings.workspace.clone(),
            task,
            what: question.to_string(),
            why: format!(
                "Workflow {} asks it in run {}.",
                self.workflow.name, self.run
            ),
            risk,
            stakes: person.then(|| Stakes {
                person: Some(true),
                ..Stakes::default()
            }),
        };
        self.board(DECIDE, &request).await
    }

    /// The run's task: added to the settings' board when it has none.
    pub async fn task(&self) -> Result<String, Failure> {
        let run = self
            .engine
            .latest(&self.run)
            .await
            .map_err(|e| failure(e.kind.name(), &e.message))?;
        if !run.data.task.is_empty() {
            return Ok(run.data.task);
        }
        let board = self
            .engine
            .approvals(true)
            .await
            .map_err(|e| failure(e.kind.name(), &e.message))?;
        let Some(board) = board else {
            return Err(failure(
                "failed_precondition",
                "an approval needs the run's task, and the module's settings name no board to add it to",
            ));
        };
        let request = AddTaskRequest {
            workspace_id: self.engine.settings.workspace.clone(),
            board,
            title: format!("Run {} of {}", self.run, self.workflow.name),
            text: self.workflow.description.clone(),
            key: Some(format!("run:{}", self.run)),
            ..Default::default()
        };
        let task: TaskRecord = self.board(ADD_TASK, &request).await?;
        self.engine
            .update_run(&self.run, |run| run.task = task.id.clone())
            .await
            .map_err(|e| failure(e.kind.name(), &e.message))?;
        Ok(task.id)
    }

    /// Board RPC `path` with `request`, in the run's scope: its response.
    pub async fn board<Req, Res>(&self, path: &str, request: &Req) -> Result<Res, Failure>
    where
        Req: Message + prost::Name,
        Res: Message + Default,
    {
        self.engine
            .board(&self.scope, path, request)
            .await
            .map_err(|e| failure(e.kind.name(), &e.message))
    }

    /// `decision` as CEL reads it.
    pub fn decision(
        &self,
        decision: &post_cortex_proto::board::Decision,
    ) -> Result<Expr, ModuleError> {
        as_message(decision, self.catalog.messages())
            .map(Expr::Message)
            .map_err(|why| ModuleError::new(ModuleErrorKind::Internal, why))
    }
}

/// Who approved decision `id`, when `event` (a `board.decision_approved`)
/// says it was.
pub fn approved(event: &BusEvent, id: &str) -> Option<String> {
    let approved = pcx_approval::heard(event)?;
    (approved.decision == id).then_some(approved.approved_by)
}

/// Who declined decision `id`, when `event` (a `board.decision_declined`)
/// says a person did.
pub fn declined(event: &BusEvent, id: &str) -> Option<String> {
    let declined = pcx_approval::declined(event)?;
    (declined.decision == id).then_some(declined.declined_by)
}

impl Engine {
    /// Board RPC `path` with `request` in `scope`: its response.
    ///
    /// # Errors
    /// The board refused or failed; `Internal` for an answer that does not
    /// read.
    pub async fn board<Req, Res>(
        &self,
        scope: &Scope,
        path: &str,
        request: &Req,
    ) -> Result<Res, ModuleError>
    where
        Req: Message + prost::Name,
        Res: Message + Default,
    {
        let message = as_message(request, self.own.messages())
            .map_err(|why| ModuleError::new(ModuleErrorKind::Internal, why))?;
        let response = self.ports.tools.call(scope, path, &message, None).await?;
        response.transcode_to::<Res>().map_err(|e| {
            ModuleError::new(
                ModuleErrorKind::Internal,
                format!("{path} answered what does not read: {e}"),
            )
        })
    }
}
