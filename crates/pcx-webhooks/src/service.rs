// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The webhooks' service (ARCHITECTURE section 8, operations, W27): a
//! source's request checked and decoded, and what its alert group becomes
//! on the board, or the run of the workflow the source names
//! (docs/modules/workflow.md, triggers), over the ports. Between the handlers and the ports.

use pcx_module_api::{ModuleError, ModuleErrorKind};
use post_cortex_proto::board::{Board, TaskRecord};
use post_cortex_proto::webhooks::{
    AlertmanagerWebhook, Handled, ReceiveRequest, ReceiveResponse, Source, SourceKind,
    WebhooksConfig,
};

use crate::alertmanager;
use crate::domain::{self, Refused, Status};
use crate::ports::{Reached, World};

fn refused(rule: Refused) -> ModuleError {
    let kind = match rule {
        Refused::Token => ModuleErrorKind::Denied,
        Refused::UnknownSource(_) => ModuleErrorKind::NotFound,
        Refused::Body(_) | Refused::Setting { .. } => ModuleErrorKind::InvalidArgument,
    };
    let field = match &rule {
        Refused::Setting { field, .. } => Some(field.clone()),
        _ => None,
    };
    let error = ModuleError::new(kind, rule.to_string());
    match field {
        Some(field) => error.with_field(field),
        None => error,
    }
}

/// `config` checked before the module starts.
///
/// # Errors
/// `InvalidArgument` naming the first wrong setting.
pub fn checked(config: WebhooksConfig) -> Result<WebhooksConfig, ModuleError> {
    domain::check(&config).map_err(refused)?;
    Ok(config)
}

/// The webhooks over `world`, as `config` sets them.
pub struct Service<'a> {
    config: &'a WebhooksConfig,
    world: &'a dyn World,
}

impl<'a> Service<'a> {
    /// The service.
    pub fn new(config: &'a WebhooksConfig, world: &'a dyn World) -> Self {
        Self { config, world }
    }

    /// What `request` does: its source's group added, commented or closed.
    ///
    /// # Errors
    /// `Denied` without the module's token; `NotFound` for an
    /// unknown source; `InvalidArgument` for a body its kind does not
    /// decode; the board's or memory's refusal or failure.
    pub async fn receive(&self, request: ReceiveRequest) -> Result<ReceiveResponse, ModuleError> {
        if !domain::authorized(&self.config.token, &request.token) {
            return Err(refused(Refused::Token));
        }
        let source = domain::source(self.config, &request.source).map_err(refused)?;
        if !source.workflow.is_empty() {
            let run = self
                .world
                .of(source)
                .run(&source.workflow, &source.name, request.body)
                .await?;
            return Ok(ReceiveResponse {
                run,
                ..answer(Handled::Run, "")
            });
        }
        match source.kind() {
            SourceKind::Alertmanager => {
                let hook = alertmanager::decode(&request.body).map_err(refused)?;
                self.alert(source, &hook).await
            }
            SourceKind::Json => Err(refused(Refused::Setting {
                field: "workflow".to_string(),
                reason: format!("JSON source {} names no workflow", source.name),
            })),
            SourceKind::Unspecified => Err(refused(Refused::Setting {
                field: "kind".to_string(),
                reason: format!("source {} has no kind", source.name),
            })),
        }
    }

    async fn alert(
        &self,
        source: &Source,
        hook: &AlertmanagerWebhook,
    ) -> Result<ReceiveResponse, ModuleError> {
        let status = domain::status(hook).map_err(refused)?;
        if hook.alerts.is_empty() {
            return Ok(answer(Handled::NoAlerts, ""));
        }
        if domain::heartbeat(source, hook) {
            return Ok(answer(Handled::Heartbeat, ""));
        }
        let world = self.world.of(source);
        let board = world.board(&source.board).await?;
        let layout = board.board.unwrap_or_default();
        let prefix = domain::group_prefix(&source.name, &hook.group_key);
        let episodes: Vec<TaskRecord> = world
            .tasks(&source.board)
            .await?
            .into_iter()
            .filter(|t| domain::of_group(t, &prefix))
            .collect();
        let open = episodes.iter().find(|t| {
            t.task
                .as_ref()
                .is_some_and(|task| domain::open(&layout, &task.column))
        });
        match (status, open) {
            (Status::Firing, Some(task)) => {
                world.comment(&task.id, &domain::firing_again(hook)).await?;
                Ok(answer(Handled::FiringAgain, &task.id))
            }
            (Status::Firing, None) => {
                let key = domain::episode_key(&prefix, episodes.len());
                let task = domain::new_task(source, hook, key);
                let added = world.add(task.clone()).await?;
                let told = tell(world.as_ref(), source, &domain::told(&task, hook)).await;
                Ok(ReceiveResponse {
                    told,
                    ..answer(Handled::Added, &added.id)
                })
            }
            (Status::Resolved, Some(task)) => {
                world.comment(&task.id, &domain::resolved(hook)).await?;
                let handled = settle(world.as_ref(), task, &layout, &source.name, hook).await?;
                Ok(answer(handled, &task.id))
            }
            (Status::Resolved, None) => Ok(answer(Handled::UnknownGroup, "")),
        }
    }
}

/// Record the incident of a resolved group whose task is `task` (on the
/// board laid out as `layout`, in its session), and close the task, as its
/// member, when the member found nothing to do. A close the board refuses
/// leaves the task open, logged: the incident is resolved and recorded
/// either way, so the source is not refused.
async fn settle(
    world: &dyn Reached,
    task: &TaskRecord,
    layout: &Board,
    source: &str,
    hook: &AlertmanagerWebhook,
) -> Result<Handled, ModuleError> {
    let thread = world.thread(&task.id).await?;
    let found = domain::findings(&thread, source);
    let data = task.task.clone().unwrap_or_default();
    world
        .record(
            &layout.session_id,
            &domain::incident(hook, &data.title, &found),
        )
        .await?;
    let (Some(member), true) = (
        data.assignee.as_deref(),
        domain::closes(&data.labels, task.held_by.is_some()),
    ) else {
        return Ok(Handled::Resolved);
    };
    let Some(done) = domain::done_column(layout) else {
        return Ok(Handled::Resolved);
    };
    match world.close(&task.id, task.version, done, member).await {
        Ok(()) => Ok(Handled::Closed),
        Err(e) => {
            tracing::warn!(task = %task.id, member, error = %e, "a resolved incident's task was not closed");
            Ok(Handled::Resolved)
        }
    }
}

fn answer(handled: Handled, task: &str) -> ReceiveResponse {
    ReceiveResponse {
        handled: handled.into(),
        task: task.to_string(),
        ..Default::default()
    }
}

/// Tell `source`'s destination `text`: whether it was told. A channel that
/// fails does not fail the request (its task is on the board): it is
/// logged.
async fn tell(world: &dyn Reached, source: &Source, text: &str) -> bool {
    if source.notify.is_empty() {
        return false;
    }
    match world.tell(&source.notify, text).await {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!(source = %source.name, error = %e, "the channel was not told of an incident");
            false
        }
    }
}

#[cfg(test)]
mod tests;
