// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What starts a run (docs/modules/workflow.md, triggers and approval):
//! a call's trigger read from the module the host says made it (the
//! scheduler's `schedule`, a webhook's `webhook`, else `call` or `manual`),
//! and an event of the bus that an `event` trigger's name and filter pick.

use std::sync::Arc;

use pcx_approval::{DECISION_APPROVED, DECISION_DECLINED};
use pcx_expr::{Type, Value as Expr};
use pcx_module_api::{ModuleError, ModuleErrorKind, Scope};
use post_cortex_proto::module::BusEvent;
use post_cortex_proto::workflow::{
    Failure, Fired, Run, RunWorkflowRequest, Webhook, block::Kind, trigger,
};
use tokio_util::sync::CancellationToken;

use crate::domain;
use crate::ports::Kept;
use crate::service::engine::{Engine, Input, Listener, Start, act, written};

/// The module whose calls start a `schedule` trigger.
const SCHEDULER: &str = "scheduler";

/// The module whose calls start a `webhook` trigger.
const WEBHOOKS: &str = "webhooks";

impl Engine {
    /// A run asked for in `scope` by `request`: its trigger by the module
    /// that called.
    ///
    /// # Errors
    /// `InvalidArgument` for a webhook's body that is not JSON; as
    /// [`Engine::start`]'s.
    pub async fn run(
        self: &Arc<Self>,
        scope: &Scope,
        request: RunWorkflowRequest,
    ) -> Result<Kept<Run>, ModuleError> {
        let author = scope.author.clone().unwrap_or_default();
        let input = Input::Map(request.input.unwrap_or_default());
        let (start, fired, input) = match scope.caller_module.as_deref() {
            Some(SCHEDULER) => (Start::Schedule, Fired::default(), input),
            Some(WEBHOOKS) => {
                let body: serde_json::Value =
                    serde_json::from_slice(&request.body).map_err(|e| {
                        ModuleError::new(
                            ModuleErrorKind::InvalidArgument,
                            format!("the webhook's body is not JSON: {e}"),
                        )
                        .with_field("body")
                    })?;
                let value = Expr::read(&Type::Dyn, &body).map_err(|why| {
                    ModuleError::new(ModuleErrorKind::InvalidArgument, why).with_field("body")
                })?;
                let fired = Fired {
                    webhook: Some(Webhook {
                        source: request.source.clone(),
                        body: Some(written(&value)?),
                    }),
                    ..Fired::default()
                };
                (Start::Webhook(request.source), fired, Input::Body(body))
            }
            _ if request.call => (Start::Call, Fired::default(), input),
            _ => (Start::Manual, Fired::default(), input),
        };
        let fired = Fired { author, ..fired };
        self.start(&request.workflow, &start, fired, &input, None)
            .await
    }

    /// The `event` triggers of the workspace's workflows read again: after
    /// the module starts and after each write.
    ///
    /// # Errors
    /// Memory failed.
    pub async fn listen(&self) -> Result<(), ModuleError> {
        let mut listeners = Vec::new();
        for workflow in self.ports.workflows.all().await? {
            let blocks = workflow.data.graph.iter().flat_map(|g| g.blocks.iter());
            for block in blocks {
                if let Some(Kind::Trigger(t)) = &block.kind
                    && let Some(trigger::Kind::Event(event)) = &t.kind
                {
                    listeners.push(Listener {
                        workspace: self.settings.workspace.clone(),
                        workflow: workflow.data.name.clone(),
                        block: block.id.clone(),
                        trigger: event.clone(),
                    });
                }
            }
        }
        // This workspace's only: the others' stay as they were read.
        let mut all = self.listeners.lock();
        all.retain(|l| l.workspace != self.settings.workspace);
        all.extend(listeners);
        Ok(())
    }

    /// An event of the bus heard: the runs that wait for one of its name
    /// read it, the `event` triggers it fires start their runs, and a
    /// decision approved or declined answers the grants it was asked for.
    ///
    /// Its payload's type URL is written as CEL reads it first
    /// ([`domain::as_heard`]), so a filter, a wait and a run's `trigger.event`
    /// read the payload's fields.
    ///
    /// An event is heard in its own workspace: by the runs and the triggers
    /// of that workspace's workflows. One of no workspace is heard in each.
    pub fn heard(self: &Arc<Self>, event: &BusEvent) {
        let event = &domain::as_heard(event);
        let everywhere = event.workspace_id.is_empty();
        for ((workspace, _), live) in self.live.lock().iter() {
            if (everywhere || *workspace == event.workspace_id)
                && live.listens.lock().contains(&event.name)
            {
                live.inbox.lock().push(event.clone());
                live.wake.notify_one();
            }
        }
        self.fire(event);
        if event.name == DECISION_APPROVED || event.name == DECISION_DECLINED {
            let engine = if everywhere {
                Arc::clone(self)
            } else {
                self.at(&event.workspace_id)
            };
            let event = event.clone();
            tokio::spawn(async move {
                if let Err(e) = engine.decided(&event).await {
                    tracing::warn!(event = %event.id, "a decision on grants was not read: {e}");
                }
            });
        }
    }

    /// The runs `event` starts: one for each `event` trigger whose name it
    /// has and whose filter holds. An event a run of a workflow emitted
    /// starts no run of that workflow, so a workflow does not feed itself.
    pub fn fire(self: &Arc<Self>, event: &BusEvent) {
        let listeners: Vec<Listener> = self
            .listeners
            .lock()
            .iter()
            .filter(|l| l.trigger.name == event.name)
            .filter(|l| event.workspace_id.is_empty() || l.workspace == event.workspace_id)
            .filter(|l| event.author != format!("workflow:{}", l.workflow))
            .cloned()
            .collect();
        for listener in listeners {
            let engine = self.at(&listener.workspace);
            let event = event.clone();
            tokio::spawn(async move {
                match engine.fires(&listener, &event).await {
                    Ok(true) => {}
                    Ok(false) => return,
                    Err(e) => {
                        tracing::warn!(workflow = %listener.workflow, block = %listener.block, "an event trigger's filter failed: {}", e.message);
                        return;
                    }
                }
                let fired = Fired {
                    author: event.author.clone(),
                    event: Some(event),
                    ..Fired::default()
                };
                let started = engine
                    .start(
                        &listener.workflow,
                        &Start::Event(listener.block.clone()),
                        fired,
                        &Input::Map(Default::default()),
                        None,
                    )
                    .await;
                if let Err(e) = started {
                    tracing::warn!(workflow = %listener.workflow, "an event did not start its run: {e}");
                }
            });
        }
    }

    /// Whether `event` passes the filter of `listener`'s trigger. The filter
    /// is compiled and the event read with the messages a run of the
    /// workflow reaches, so it reads the payload of an event of any module
    /// the workflow reaches, not of the board's only. A trigger with no
    /// filter asks the host nothing.
    ///
    /// # Errors
    /// The host failed; the filter does not compile, failed or gave no
    /// bool; the event does not read.
    async fn fires(&self, listener: &Listener, event: &BusEvent) -> Result<bool, ModuleError> {
        if listener.trigger.filter.trim().is_empty() {
            return Ok(true);
        }
        let scope = self.scope(&listener.workflow, 0, CancellationToken::new());
        let catalog = self.service().catalog(&scope).await?;
        let refused =
            |failed: Failure| ModuleError::new(ModuleErrorKind::InvalidArgument, failed.message);
        let message = act::event(event, &catalog)?;
        domain::Fires::new(listener.trigger.clone(), &catalog)
            .map_err(refused)?
            .fires(&event.name, Expr::Message(message))
            .map_err(refused)
    }
}
