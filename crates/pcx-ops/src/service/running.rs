// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! An approved action run: its target checked again (it may have changed
//! since it was asked), the change sent, the action written with how it
//! ended, a comment on its task and the channel told. A failure of the
//! cluster is a failed action, not an error: it is written and said. An
//! action whose decision a person declined, or its asker withdrew, is
//! given up. The module looks at those that wait: one answered while it
//! was down is settled, one no one answers is said again and then given up.

use pcx_documents::Item;
use pcx_module_api::ModuleError;
use post_cortex_proto::ops::{Action, ActionAnswer, Environment, State};

use crate::domain::{self, Wanted};
use crate::paths;
use crate::ports::Patch;
use crate::service::{Service, answer, refused, stamp};

impl Service<'_> {
    /// A look at every action that still waits: how many it settled. One
    /// whose decision was answered runs or is given up (the write of a
    /// decision answered while the module was down is not heard again);
    /// one no one answered is said again once on the channel, and later
    /// its decision is withdrawn in its asker's name, which gives it up at
    /// the next look or when that write is heard. An action that fails
    /// here is said and does not hold the others.
    ///
    /// # Errors
    /// Memory failed reading the waiting actions.
    pub async fn look(&self) -> Result<usize, ModuleError> {
        let waiting = Action {
            state: State::Waiting.into(),
            ..Action::default()
        };
        let kept: Vec<Item<Action>> = self.actions().all(Some((&waiting, &["state"][..]))).await?;
        let mut settled = 0;
        for action in kept {
            let decision = action.data.decision.clone();
            let looked = match self.decided(&decision).await {
                Ok(Some(_)) => {
                    settled += 1;
                    Ok(())
                }
                Ok(None) => self.unanswered(action).await,
                Err(e) => Err(e),
            };
            if let Err(e) = looked {
                tracing::warn!(%decision, error = %e, "a waiting action was not looked at");
            }
        }
        Ok(settled)
    }

    /// What the action `kept`, which no one answered yet, is owed for the
    /// time it has waited: said again once, or its decision withdrawn.
    async fn unanswered(&self, kept: Item<Action>) -> Result<(), ModuleError> {
        let now = self.clock.now();
        match domain::waited(self.config, &kept.data, now) {
            domain::Waited::Short => Ok(()),
            domain::Waited::Remind(said) => {
                let reminded = Action {
                    reminded: Some(stamp(now)),
                    ..kept.data.clone()
                };
                self.actions()
                    .change(&kept.document.id, kept.document.version, &reminded, "")
                    .await?;
                if self.config.notify.is_empty() {
                    return Ok(());
                }
                self.teller.tell(&self.config.notify, &said).await
            }
            domain::Waited::GiveUp(why) => {
                let action = &kept.data;
                self.board
                    .withdraw(
                        &action.asked_by,
                        &action.workspace_id,
                        &action.decision,
                        &why,
                    )
                    .await
            }
        }
    }

    /// The waiting action `kept` given up because of `why` (a person
    /// declined its decision, or who asked withdrew it): written as
    /// declined and said on its task, its target free for another action.
    ///
    /// # Errors
    /// Memory failed writing the action.
    pub async fn give_up(
        &self,
        kept: Item<Action>,
        why: &str,
    ) -> Result<ActionAnswer, ModuleError> {
        let declined = Action {
            state: State::Declined.into(),
            result: why.to_string(),
            ..kept.data.clone()
        };
        let ended = self
            .actions()
            .change(&kept.document.id, kept.document.version, &declined, "")
            .await?;
        let said = format!("Not done: {}: {why}", ended.data.what);
        if let Err(e) = self
            .board
            .comment(&ended.data.workspace_id, &ended.data.task, &said)
            .await
        {
            tracing::warn!(error = %e, "an action given up was not commented");
        }
        Ok(answer(ended, String::new()))
    }

    /// Run the approved action `kept` in `env`; `on_approval` when a
    /// person approved it after its asker's turn, so its end names the
    /// asker, who checks what it did.
    ///
    /// # Errors
    /// Memory failed writing the action.
    pub async fn run(
        &self,
        env: &Environment,
        kept: Item<Action>,
        on_approval: bool,
    ) -> Result<ActionAnswer, ModuleError> {
        let running = Action {
            state: State::Running.into(),
            ran: Some(stamp(self.clock.now())),
            ..kept.data.clone()
        };
        let running = self
            .actions()
            .change(&kept.document.id, kept.document.version, &running, "")
            .await?;
        let action = &running.data;
        let (state, result) = match self.change(env, action).await {
            Ok(result) => (State::Done, result),
            Err(e) => (State::Failed, e.message),
        };
        let ended = Action {
            state: state.into(),
            result: result.clone(),
            ..running.data.clone()
        };
        let ended = self
            .actions()
            .change(&running.document.id, running.document.version, &ended, "")
            .await?;
        let said = match state {
            State::Done => format!("Done: {} ({result})", ended.data.what),
            _ => format!("Not done: {}: {result}", ended.data.what),
        };
        let comment = if on_approval {
            format!("{said}\n\n{}", domain::look_again(&ended.data.asked_by))
        } else {
            said.clone()
        };
        // The record is the action's document; a comment or a channel that
        // fails is logged, not the action's failure.
        if let Err(e) = self
            .board
            .comment(&ended.data.workspace_id, &ended.data.task, &comment)
            .await
        {
            tracing::warn!(error = %e, "an action's end was not commented");
        }
        if !self.config.notify.is_empty()
            && let Err(e) = self.teller.tell(&self.config.notify, &said).await
        {
            tracing::warn!(error = %e, "an action's end was not told");
        }
        Ok(answer(ended, String::new()))
    }

    /// Check `action`'s target again and change it: what it did.
    async fn change(&self, env: &Environment, action: &Action) -> Result<String, ModuleError> {
        let wanted = domain::wanted(action).map_err(refused)?;
        let planned = self.planned(env, &wanted).await?;
        let before = &planned.now;
        match &wanted {
            Wanted::Restart { namespace, name } => {
                let patch = Patch::Deployment(domain::restart(self.clock.now()));
                self.cluster
                    .patch(env, &paths::deployment(namespace, name), patch)
                    .await?;
                Ok(format!("rolled out; it had {before}"))
            }
            Wanted::DeletePod { namespace, name } => {
                self.cluster
                    .delete(env, &paths::pod(namespace, name))
                    .await?;
                Ok("deleted; its controller makes it again".to_string())
            }
            Wanted::CleanJobs { namespace } => {
                let jobs = self.cluster.list(env, &paths::jobs(namespace)).await?;
                let failed = domain::failed_jobs(&jobs);
                for job in &failed {
                    self.cluster
                        .delete(env, &paths::job(namespace, job))
                        .await?;
                }
                Ok(format!("deleted {}", failed.join(", ")))
            }
            Wanted::Scale {
                namespace,
                name,
                replicas,
            } => {
                let patch = Patch::Deployment(domain::scale(*replicas));
                self.cluster
                    .patch(env, &paths::scale(namespace, name), patch)
                    .await?;
                Ok(format!("{before} before, {replicas} now asked"))
            }
            Wanted::ResizeVolume {
                namespace,
                name,
                gibibytes,
            } => {
                let patch = Patch::Claim(domain::grow(*gibibytes));
                self.cluster
                    .patch(env, &paths::claim(namespace, name), patch)
                    .await?;
                Ok(format!("{before} before, {gibibytes}Gi now asked"))
            }
            Wanted::NewAllocation { .. } => {
                self.nomad.stop(env, &planned.allocation).await?;
                Ok("stopped; Nomad places a new one".to_string())
            }
            Wanted::RunPeriodic { job } => {
                self.nomad.launch(env, job).await?;
                Ok("launched".to_string())
            }
            Wanted::RevertJobs { .. } => self.revert_all(env, &action.reverts, &planned).await,
        }
    }
}
