// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Ops' service (ARCHITECTURE section 8, operations, W31): an action asked
//! is checked (its request, its target as it is now, the brakes), recorded
//! as a decision with risk operations on its task and kept (`ops/action`);
//! it runs at once when the team's trust approved the decision, else when
//! a person approves it, its target checked again first (`running`); a
//! Nomad action's target in `nomad`. Between the handlers (and the
//! module's events) and the ports.

use chrono::{DateTime, Utc};
use pcx_documents::{Documents, Item, Typed};
use pcx_module_api::{ModuleError, ModuleErrorKind};
use post_cortex_proto::ops::{
    Action, ActionAnswer, ActionList, Ask, Environment, Kind, OpsConfig, State,
};
use prost_types::Timestamp;

use crate::domain::{self, ACTION, Planned, Refused, Wanted};
use crate::paths;
use crate::ports::{Board, Clock, Cluster, Decision, Nomad, Teller};

/// Actions a listing answers when the request names no limit.
const LISTED: usize = 20;

fn refused(rule: Refused) -> ModuleError {
    let kind = match rule {
        Refused::UnknownEnvironment(_) => ModuleErrorKind::NotFound,
        Refused::Brake(_) | Refused::Target(_) | Refused::Cluster { .. } => {
            ModuleErrorKind::FailedPrecondition
        }
        Refused::Setting { .. }
        | Refused::Empty(_)
        | Refused::Name(_)
        | Refused::NomadName(_)
        | Refused::AllocationId
        | Refused::Bounds(_) => ModuleErrorKind::InvalidArgument,
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
pub fn checked(config: OpsConfig) -> Result<OpsConfig, ModuleError> {
    domain::check(&config).map_err(refused)?;
    Ok(config)
}

fn stamp(time: DateTime<Utc>) -> Timestamp {
    Timestamp {
        seconds: time.timestamp(),
        nanos: 0,
    }
}

fn answer(item: Item<Action>, would: String) -> ActionAnswer {
    ActionAnswer {
        id: item.document.id,
        action: Some(item.data),
        would,
    }
}

/// An action asked: what every action asks, and what it wants done.
#[derive(Debug, Clone, PartialEq)]
pub struct Asked {
    /// What every action asks.
    pub ask: Ask,
    /// What it wants done.
    pub wanted: Wanted,
}

/// The target `namespace/name` of an action on an object, each a
/// Kubernetes name.
fn object(namespace: &str, name: &str) -> Result<String, ModuleError> {
    Ok(format!(
        "{}/{}",
        domain::named("namespace", namespace).map_err(refused)?,
        domain::named("object", name).map_err(refused)?
    ))
}

/// Ops over its ports, as `config` sets it.
pub struct Service<'a> {
    /// The settings.
    pub config: &'a OpsConfig,
    /// Its actions' documents, in its workspace.
    pub docs: &'a dyn Documents,
    /// The Kubernetes clusters.
    pub cluster: &'a dyn Cluster,
    /// The Nomad clusters.
    pub nomad: &'a dyn Nomad,
    /// The board.
    pub board: &'a dyn Board,
    /// The channel.
    pub teller: &'a dyn Teller,
    /// The time.
    pub clock: &'a dyn Clock,
}

impl Service<'_> {
    fn actions(&self) -> Typed<'_> {
        Typed::new(self.docs, ACTION)
    }

    /// The actions asked, the newest first, at most `limit` (0: 20).
    ///
    /// # Errors
    /// Memory failed.
    pub async fn list(&self, limit: usize) -> Result<ActionList, ModuleError> {
        let limit = if limit == 0 { LISTED } else { limit };
        let (newest, _) = self
            .actions()
            .after::<Action>(None, None, limit, |_| true)
            .await?;
        Ok(ActionList {
            actions: newest
                .into_iter()
                .map(|a| answer(a, String::new()))
                .collect(),
        })
    }

    /// What the brakes weigh in `environment`: its actions that wait, and
    /// those written within the brakes' window (docs/host.md, pages in a
    /// module: never every action).
    async fn braking(&self, environment: &str) -> Result<Vec<Action>, ModuleError> {
        let waiting = Action {
            environment: environment.to_string(),
            state: State::Waiting.into(),
            ..Action::default()
        };
        let mut weighed: Vec<Item<Action>> = self
            .actions()
            .all(Some((&waiting, &["environment", "state"][..])))
            .await?;
        let since = (self.clock.now() - domain::brake_window(self.config)).timestamp();
        let mut recent = Vec::new();
        self.actions()
            .scan(
                Some((&waiting, &["environment"][..])),
                |item: Item<Action>| {
                    let fresh = item
                        .document
                        .updated_at
                        .as_ref()
                        .is_some_and(|at| at.seconds >= since);
                    if fresh {
                        recent.push(item);
                    }
                    fresh
                },
            )
            .await?;
        weighed.retain(|w| recent.iter().all(|r| r.document.id != w.document.id));
        weighed.extend(recent);
        Ok(weighed.into_iter().map(|a| a.data).collect())
    }

    /// Ask `asked` as `asker`: what it would do (a dry run), or the action
    /// kept and its decision recorded; run at once when the team's trust
    /// approved it.
    ///
    /// # Errors
    /// A request the rules refuse; a target that is not what the action is
    /// for or a brake (`FailedPrecondition`); the cluster, the board or
    /// memory failed.
    pub async fn act(&self, asker: &str, asked: Asked) -> Result<ActionAnswer, ModuleError> {
        domain::asked(&asked.ask).map_err(refused)?;
        let env = domain::environment(self.config, &asked.ask.environment).map_err(refused)?;
        let planned = self.planned(env, &asked.wanted).await?;
        let what = domain::what(&planned);
        if asked.ask.dry_run {
            return Ok(ActionAnswer {
                id: String::new(),
                action: None,
                would: format!("would {what}; now: {}", planned.now),
            });
        }
        let kept = self.braking(&env.name).await?;
        for target in domain::targets(&planned.target, &planned.reverts) {
            domain::brake(self.config, &kept, &env.name, target, self.clock.now())
                .map_err(refused)?;
        }
        let decided = self
            .board
            .decide(
                asker,
                Decision {
                    workspace: asked.ask.workspace_id.clone(),
                    task: asked.ask.task.clone(),
                    what: what.clone(),
                    why: asked.ask.why.clone(),
                    stakes: domain::stakes(planned.kind, &env.name, &planned.target),
                },
            )
            .await?;
        let action = Action {
            kind: planned.kind.into(),
            what,
            environment: env.name.clone(),
            target: planned.target,
            amount: planned.amount,
            why: asked.ask.why,
            workspace_id: asked.ask.workspace_id,
            task: asked.ask.task,
            decision: decided.id,
            asked_by: asker.to_string(),
            state: State::Waiting.into(),
            result: String::new(),
            asked: Some(stamp(self.clock.now())),
            ran: None,
            reminded: None,
            reverts: planned.reverts,
            allocation: planned.allocation,
        };
        let kept = self.actions().create(&action, "").await?;
        if decided.pending {
            return Ok(answer(kept, String::new()));
        }
        self.run(env, kept, false).await
    }

    /// Decision `id` was written: its action, still waiting, runs when it
    /// is approved and is given up when a person declined it or who asked
    /// withdrew it; none when it is no action's or still waits.
    ///
    /// # Errors
    /// The board, the cluster or memory failed.
    pub async fn decided(&self, id: &str) -> Result<Option<ActionAnswer>, ModuleError> {
        let of = Action {
            decision: id.to_string(),
            ..Action::default()
        };
        let found = self.actions().all(Some((&of, &["decision"][..]))).await?;
        let Some(waiting) = found.into_iter().find(|a| a.data.state() == State::Waiting) else {
            return Ok(None);
        };
        let action = &waiting.data;
        let Some(decision) = self
            .board
            .decision(&action.workspace_id, &action.task, id)
            .await?
        else {
            return Ok(None);
        };
        if decision.pending {
            return Ok(None);
        }
        let decision = decision.decision.unwrap_or_default();
        if decision.approved_by.is_some() {
            let env = domain::environment(self.config, &action.environment).map_err(refused)?;
            return self.run(env, waiting, true).await.map(Some);
        }
        match domain::given_up(&decision) {
            Some(why) => self.give_up(waiting, &why).await.map(Some),
            None => Ok(None),
        }
    }

    /// What `wanted` is to do in `env`: its target read as it is now and
    /// checked for the action, with how a person reads it.
    async fn planned(&self, env: &Environment, wanted: &Wanted) -> Result<Planned, ModuleError> {
        domain::cluster(env, wanted).map_err(refused)?;
        match wanted {
            Wanted::Restart { namespace, name } => {
                let target = object(namespace, name)?;
                let path = paths::deployment(namespace, name);
                let d = self.cluster.get(env, &path).await?;
                let now = format!("{} replicas", d.spec.map_or(0, |s| s.replicas));
                Ok(Planned::of(Kind::Restart, target, now))
            }
            Wanted::DeletePod { namespace, name } => {
                let target = object(namespace, name)?;
                let pod = self.cluster.get(env, &paths::pod(namespace, name)).await?;
                domain::deletable(&pod).map_err(refused)?;
                let now = "a controller makes it again".to_string();
                Ok(Planned::of(Kind::DeletePod, target, now))
            }
            Wanted::CleanJobs { namespace } => {
                let ns = domain::named("namespace", namespace).map_err(refused)?;
                let jobs = self.cluster.list(env, &paths::jobs(ns)).await?;
                let failed = domain::failed_jobs(&jobs);
                if failed.is_empty() {
                    return Err(refused(Refused::Target(format!("{ns} has no failed jobs"))));
                }
                let now = format!("failed: {}", failed.join(", "));
                Ok(Planned::of(Kind::CleanJobs, ns.to_string(), now))
            }
            Wanted::Scale {
                namespace,
                name,
                replicas,
            } => {
                let target = object(namespace, name)?;
                let replicas = domain::replicas(env, *replicas).map_err(refused)?;
                let path = paths::deployment(namespace, name);
                let d = self.cluster.get(env, &path).await?;
                let now = d.spec.map_or(0, |s| s.replicas);
                if now == replicas {
                    return Err(refused(Refused::Target(format!(
                        "{target} has {now} replicas already"
                    ))));
                }
                Ok(Planned {
                    amount: replicas,
                    ..Planned::of(Kind::Scale, target, format!("{now} replicas"))
                })
            }
            Wanted::ResizeVolume {
                namespace,
                name,
                gibibytes,
            } => {
                let target = object(namespace, name)?;
                let claim = self
                    .cluster
                    .get(env, &paths::claim(namespace, name))
                    .await?;
                let size = domain::growable(&claim, *gibibytes).map_err(refused)?;
                Ok(Planned {
                    amount: *gibibytes,
                    ..Planned::of(Kind::ResizeVolume, target, format!("{size}Gi"))
                })
            }
            Wanted::NewAllocation { allocation } => self.new_allocation(env, allocation).await,
            Wanted::RunPeriodic { job } => self.periodic(env, job).await,
            Wanted::RevertJobs { jobs } => self.reverts(env, jobs).await,
        }
    }
}

mod nomad;
mod running;

#[cfg(test)]
mod tests;
