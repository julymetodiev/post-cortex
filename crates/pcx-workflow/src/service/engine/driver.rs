// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A run's driver (docs/modules/workflow.md, the engine): one task a run,
//! which asks the domain what the journal makes next, starts the attempts
//! that are due, settles the steps that wait when their time, event,
//! decision or called run comes, and ends the run when the domain says.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use chrono::{DateTime, TimeDelta, Utc};
use parking_lot::Mutex;
use pcx_approval::{DECISION_APPROVED, DECISION_DECLINED};
use pcx_expr::Value as Expr;
use pcx_module_api::{ModuleError, ModuleErrorKind};
use post_cortex_proto::module::BusEvent;
use post_cortex_proto::workflow::{Failure, RunStatus, Step};
use prost::Message;
use tokio::sync::Notify;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::domain::{
    self, Awaited, Catalog, Journal, Next, Plan, as_message, failure, kinds, open,
};
use crate::ports::Kept;
use crate::service::engine::act;
use crate::service::engine::written;
use crate::service::engine::{Engine, IDLE, Key, Live, STEPS, WALL, key_of, run_input};

impl Engine {
    /// Drive run `id` in a task of its own, unless one drives it.
    pub fn drive(self: &Arc<Self>, id: String) {
        let Some(live) = self.claim(&id) else {
            return;
        };
        let engine = Arc::clone(self);
        tokio::spawn(async move {
            if let Err(error) = engine.driven(&id, &live).await {
                tracing::warn!(run = %id, "the run's driver stopped: {error}");
                match engine.ports.runs.get(&id).await {
                    Ok(Some(run)) if open(&run.data) => {
                        let failed = failure(error.kind.name(), &error.message);
                        if let Err(e) = engine
                            .end(&run, RunStatus::Failed, None, Some(failed))
                            .await
                        {
                            tracing::warn!(run = %id, "the run could not be marked failed: {e}");
                        }
                    }
                    Ok(_) => {}
                    Err(e) => tracing::warn!(run = %id, "the run could not be read: {e}"),
                }
            }
            engine.live.lock().remove(&engine.live_key(&id));
        });
    }

    /// Run `id` marked driven: its handle; none when it is driven
    /// already.
    pub fn claim(&self, id: &str) -> Option<Arc<Live>> {
        let key = self.live_key(id);
        let mut all = self.live.lock();
        if all.contains_key(&key) {
            return None;
        }
        let live = Arc::new(Live {
            wake: Arc::new(Notify::new()),
            cancel: CancellationToken::new(),
            inbox: Mutex::new(Vec::new()),
            listens: Mutex::new(HashSet::new()),
        });
        all.insert(key, Arc::clone(&live));
        Some(live)
    }

    /// The loop of run `id`: plan, act, wait, until it ends.
    pub async fn driven(self: &Arc<Self>, id: &str, live: &Arc<Live>) -> Result<(), ModuleError> {
        let mut run =
            self.ports.runs.get(id).await?.ok_or_else(|| {
                ModuleError::new(ModuleErrorKind::NotFound, format!("no run {id}"))
            })?;
        let workflow = self
            .ports
            .workflows
            .at(&run.data.workflow_id, run.data.version)
            .await?
            .ok_or_else(|| {
                ModuleError::new(
                    ModuleErrorKind::NotFound,
                    format!("the version run {id} runs is gone"),
                )
            })?
            .data;
        let scope = self.scope(&workflow.name, run.data.call_depth, live.cancel.clone());
        let (catalog, checked) = match self.service().checked(&scope, &workflow).await {
            Ok(checked) => checked,
            Err(e) => {
                let failed = failure(
                    e.kind.name(),
                    &format!("the workflow does not check now: {}", e.message),
                );
                self.end(&run, RunStatus::Failed, None, Some(failed))
                    .await?;
                return Ok(());
            }
        };
        let input = run_input(&run.data, &catalog)?;
        let fired = Expr::Message(
            as_message(
                &run.data.trigger.clone().unwrap_or_default(),
                catalog.messages(),
            )
            .map_err(|why| ModuleError::new(ModuleErrorKind::Internal, why))?,
        );
        let trigger = run
            .data
            .trigger
            .as_ref()
            .map(|t| t.block.clone())
            .unwrap_or_default();
        let budgets = workflow.budgets.unwrap_or_default();
        let max_steps = if budgets.steps == 0 {
            STEPS
        } else {
            budgets.steps
        };
        let wall = budgets
            .wall
            .and_then(|w| TimeDelta::try_seconds(w.seconds))
            .filter(|w| *w > TimeDelta::zero())
            .unwrap_or(WALL);
        let started = run
            .data
            .started_at
            .as_ref()
            .and_then(domain::time)
            .unwrap_or_else(|| self.ports.clock.now());
        let mut marked = (run.data.waits_until.is_none()
            && run.data.status() == RunStatus::Running)
            .then_some((RunStatus::Running, None));
        let mut steps = self.steps(id).await?;
        // Each step's output is read once, when the journal records it, not
        // again for each block after it (docs/modules/workflow.md, data
        // blocks, speed).
        let mut journal = Journal::new(steps.values().map(|k| k.data.clone()), catalog.messages());
        if !self.dry() {
            let kept: Vec<Step> = steps.values().map(|k| k.data.clone()).collect();
            let missed = self.missed(&scope, &workflow, &kept).await;
            live.inbox.lock().extend(missed);
        }
        let mut tasks: JoinSet<(Key, Result<Kept<Step>, ModuleError>)> = JoinSet::new();
        let mut inflight: HashSet<Key> = HashSet::new();
        let actor = Arc::new(act::Actor {
            engine: Arc::clone(self),
            workflow: workflow.clone(),
            catalog: catalog.clone(),
            scope: scope.clone(),
            run: id.to_string(),
        });
        loop {
            let now = self.ports.clock.now();
            if live.cancel.is_cancelled() {
                tasks.abort_all();
                let run = self.latest(id).await?;
                self.end(
                    &run,
                    RunStatus::Cancelled,
                    None,
                    Some(failure(kinds::CANCELLED, "cancelled")),
                )
                .await?;
                return Ok(());
            }
            run = self.latest(id).await?;
            if !open(&run.data) {
                return Ok(());
            }
            if now - started > wall {
                return self
                    .over(
                        &run,
                        &format!("it ran past its wall time of {} s", wall.num_seconds()),
                    )
                    .await;
            }
            if steps.len() >= max_steps as usize {
                return self
                    .over(&run, &format!("it wrote its {max_steps} steps"))
                    .await;
            }
            if budgets.tokens > 0
                && run
                    .data
                    .used
                    .as_ref()
                    .is_some_and(|u| u.tokens > budgets.tokens)
            {
                return self
                    .over(&run, &format!("it spent its {} tokens", budgets.tokens))
                    .await;
            }
            let next = Plan {
                workflow: &workflow,
                checked: &checked,
                catalog: &catalog,
                input: &input,
                trigger: (&trigger, &fired),
                journal: &journal,
                now,
            }
            .next();
            let acts = match next {
                Next::Succeeded(output) => {
                    self.end(&run, RunStatus::Succeeded, Some(written(&output)?), None)
                        .await?;
                    return Ok(());
                }
                Next::Failed(failed) => {
                    self.end(&run, RunStatus::Failed, None, Some(failed))
                        .await?;
                    return Ok(());
                }
                Next::Act(acts) => acts,
            };
            let heard: Vec<BusEvent> = std::mem::take(&mut *live.inbox.lock());
            let mut earliest: Option<DateTime<Utc>> = None;
            let mut listens = HashSet::new();
            // A waiting step closed: plan again at once.
            let mut closed = false;
            for act in acts {
                let key = (act.at.clone(), act.block.clone(), act.attempt);
                if inflight.contains(&key) {
                    continue;
                }
                if let domain::Work::Await(awaited) = &act.work {
                    let settled = self
                        .settled(awaited, &catalog, &heard, now, &mut listens, &mut earliest)
                        .await?;
                    if let Some(end) = settled {
                        let existing = steps.get(&key).cloned();
                        let ended = actor.close(existing, &act, end).await?;
                        journal.record(ended.data.clone());
                        steps.insert(key, ended);
                        closed = true;
                    }
                    continue;
                }
                if let Some(not_before) = act.not_before.filter(|t| *t > now) {
                    earliest = Some(earliest.map_or(not_before, |e| e.min(not_before)));
                    continue;
                }
                inflight.insert(key.clone());
                let existing = steps.get(&key).cloned();
                let actor = Arc::clone(&actor);
                tasks.spawn(async move {
                    let done = actor.act(existing, act).await;
                    (key, done)
                });
            }
            *live.listens.lock() = listens;
            if closed {
                continue;
            }
            let status = if inflight.is_empty() {
                RunStatus::Waiting
            } else {
                RunStatus::Running
            };
            // Written when it changes: a run's status does not, step after step.
            let mark = (status, earliest.filter(|_| inflight.is_empty()));
            if marked != Some(mark) {
                self.mark(&run, mark.0, mark.1).await?;
                marked = Some(mark);
            }
            let sleep = earliest.map_or(IDLE, |t| (t - now).to_std().unwrap_or_default().min(IDLE));
            tokio::select! {
                Some(done) = tasks.join_next(), if !tasks.is_empty() => {
                    let (key, written) = done.map_err(|e| ModuleError::new(ModuleErrorKind::Internal, format!("a step's task failed: {e}")))?;
                    inflight.remove(&key);
                    let written = written?;
                    journal.record(written.data.clone());
                    steps.insert(key_of(&written.data), written);
                }
                // A wake brings an event to the inbox or a called run's end;
                // the run's steps are this loop's own, already in the
                // journal, and are not read again.
                () = live.wake.notified() => {}
                () = tokio::time::sleep(sleep) => {}
                () = live.cancel.cancelled() => {}
                () = self.stopping.cancelled() => {
                    tasks.abort_all();
                    return Ok(());
                }
            }
        }
    }

    /// Whether what `awaited` waits for has come: the step's end (its
    /// output, or its failure), else none, the events it listens for into
    /// `listens` and its time into `earliest`.
    pub async fn settled(
        &self,
        awaited: &Awaited,
        catalog: &Catalog,
        heard: &[BusEvent],
        now: DateTime<Utc>,
        listens: &mut HashSet<String>,
        earliest: &mut Option<DateTime<Utc>>,
    ) -> Result<Option<Result<Expr, Failure>>, ModuleError> {
        let mut deadline = |until: DateTime<Utc>| {
            *earliest = Some(earliest.map_or(until, |e| e.min(until)));
        };
        Ok(match awaited {
            Awaited::Time { until } => {
                if *until <= now {
                    Some(Ok(Expr::Null))
                } else {
                    deadline(*until);
                    None
                }
            }
            Awaited::Event {
                name,
                filter,
                until,
            } => {
                listens.insert(name.clone());
                let mut found = None;
                for event in heard.iter().filter(|e| &e.name == name) {
                    let message = act::event(event, catalog)?;
                    let passes = match filter {
                        None => Ok(true),
                        Some(f) => f.passes(Expr::Message(message.clone())),
                    };
                    match passes {
                        Ok(true) => {
                            found = Some(Ok(Expr::Message(message)));
                            break;
                        }
                        Ok(false) => {}
                        Err(failed) => {
                            found = Some(Err(failed));
                            break;
                        }
                    }
                }
                match (found, until) {
                    (Some(end), _) => Some(end),
                    (None, Some(until)) if *until <= now => Some(Err(failure(
                        kinds::TIMEOUT,
                        &format!("no {name} came in time"),
                    ))),
                    (None, Some(until)) => {
                        deadline(*until);
                        None
                    }
                    (None, None) => None,
                }
            }
            Awaited::Decision { id } => {
                listens.insert(DECISION_APPROVED.to_string());
                listens.insert(DECISION_DECLINED.to_string());
                // Approved: the step's output is who approved it. Declined
                // by a person: the step fails, and its `error` edge runs.
                heard.iter().find_map(|e| {
                    act::approved(e, id)
                        .map(|approved_by| Ok(Expr::string(approved_by)))
                        .or_else(|| {
                            act::declined(e, id).map(|by| {
                                Err(failure(kinds::DECLINED, &format!("declined by {by}")))
                            })
                        })
                })
            }
            Awaited::Run { id } => match self.ports.runs.get(id).await? {
                None => Some(Err(failure(
                    "not_found",
                    &format!("the called run {id} is gone"),
                ))),
                Some(child) => match RunStatus::try_from(child.data.status) {
                    Ok(RunStatus::Succeeded) => Some(match &child.data.output {
                        Some(output) => {
                            domain::read_value(output, catalog.messages()).map_err(|why| {
                                failure("internal", &format!("the called run's output: {why}"))
                            })
                        }
                        None => Ok(Expr::Null),
                    }),
                    Ok(RunStatus::Failed | RunStatus::Cancelled) => {
                        Some(Err(child.data.failure.clone().unwrap_or_else(|| {
                            failure("internal", "the called run failed")
                        })))
                    }
                    _ => None,
                },
            },
        })
    }

    /// The steps of run `id`, by key, their large outputs read from their
    /// blobs.
    pub async fn steps(&self, id: &str) -> Result<BTreeMap<Key, Kept<Step>>, ModuleError> {
        let mut steps = BTreeMap::new();
        for mut step in self.ports.runs.steps(id).await? {
            if let Some(blob) = &step.data.output_blob {
                let bytes = self.ports.blobs.get(&blob.sha256).await?;
                let value =
                    post_cortex_proto::workflow::Value::decode(bytes.as_slice()).map_err(|e| {
                        ModuleError::new(
                            ModuleErrorKind::Internal,
                            format!("a step's output blob does not read: {e}"),
                        )
                    })?;
                step.data.output = Some(value);
            }
            steps.insert(key_of(&step.data), step);
        }
        Ok(steps)
    }
}
