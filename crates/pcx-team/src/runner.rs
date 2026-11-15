// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The team's background work: a change of the board, read off its event,
//! wakes members; each member works on a task one turn at a time, with at
//! most one more waiting, each turn within the settings' deadline and
//! tried again as they say; work that failed its tries blocks its task
//! (W32); work that needs a runner that is offline waits, labeled, until
//! the runner connects (docs/connect.md section 7). All of it stops with
//! the module, and what was cut short goes on when the module starts again.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use pcx_documents::{HostDocuments, HostSessions};
use pcx_module_api::agent::TurnEnd;
use pcx_module_api::grpc::GrpcChannel;
use pcx_module_api::{ModuleError, ModuleHost, Scope};
use post_cortex_proto::team::TeamConfig;
use tokio_util::sync::CancellationToken;

use crate::adapters::HostWorld;
use crate::ending::{self, Ending};
use crate::going::{self, scope};
use crate::ports::Labels;
use crate::progress::{Busy, Progress};
use crate::wake::Wake;
use crate::work::Worker;

/// Longest a change takes to read.
const READ_DEADLINE: Duration = Duration::from_secs(30);

/// The most turns the members take on one task while the module runs: a
/// guard against members waking each other without end.
pub const MAX_TURNS_PER_TASK: u32 = 20;

/// A member's work on a task: what stops it, and the wake that waits.
struct Run {
    stop: CancellationToken,
    waits: Option<Wake>,
    progress: Arc<Progress>,
}

type Key = (String, String, String);

/// The background work of a running team module.
pub struct Runner {
    host: Arc<dyn ModuleHost>,
    /// Its channel to the services of the modules it uses.
    channel: GrpcChannel,
    /// Its settings: a turn's deadline and the tries.
    config: TeamConfig,
    stop: CancellationToken,
    /// Running work, by (workspace, task, member), with the wake that
    /// waits after it.
    running: Mutex<HashMap<Key, Run>>,
    /// Turns taken per (workspace, task).
    taken: Mutex<HashMap<(String, String), u32>>,
    /// Work waiting for a runner, by workspace, while the module runs.
    waiting: Mutex<Vec<(String, Wake)>>,
}

impl Runner {
    /// The work over `host`, as `config` sets it.
    pub fn new(host: Arc<dyn ModuleHost>, channel: GrpcChannel, config: TeamConfig) -> Arc<Self> {
        Arc::new(Self {
            channel,
            host,
            config,
            stop: CancellationToken::new(),
            running: Mutex::default(),
            taken: Mutex::default(),
            waiting: Mutex::default(),
        })
    }

    /// The turns of `workspace` that run now.
    pub fn busy(&self, workspace: &str) -> Vec<Busy> {
        self.running
            .lock()
            .iter()
            .filter(|((at, _, _), _)| at == workspace)
            .map(|((_, task, member), run)| Busy {
                task: task.clone(),
                member: member.clone(),
                quiet_seconds: run.progress.quiet().as_secs(),
                received: run.progress.received(),
            })
            .collect()
    }

    /// Stop `member`'s work on `task` of `workspace`, and the wake that
    /// waits after it; whether a turn of it was running.
    pub fn stop_work(&self, workspace: &str, task: &str, member: &str) -> bool {
        let key = (workspace.to_string(), task.to_string(), member.to_string());
        let mut running = self.running.lock();
        let Some(run) = running.get_mut(&key) else {
            return false;
        };
        run.waits = None;
        run.stop.cancel();
        true
    }

    /// Stop every turn and every wait.
    pub fn stop(&self) {
        self.stop.cancel();
    }

    /// A runner connected: the work waiting for it goes on, its task's
    /// label taken off first.
    pub fn resume(self: &Arc<Self>) {
        let waiting = std::mem::take(&mut *self.waiting.lock());
        for (workspace, wake) in waiting {
            let runner = Arc::clone(self);
            tokio::spawn(async move {
                if let Err(error) = runner
                    .unlabel(&workspace, &wake.task, &[ending::WAITING])
                    .await
                {
                    tracing::warn!(task = %wake.task, %error, "a task's waiting label was not taken off");
                }
                runner.wake(&workspace, wake);
            });
        }
    }

    /// The team started: the turns a stop cut short go on, each in its
    /// session, no try spent (docs/connect.md section 7).
    pub fn recover(self: &Arc<Self>) {
        let runner = Arc::clone(self);
        tokio::spawn(async move {
            let found = going::cut_short(&runner.channel, &runner.stop).await;
            for (workspace, wake) in found {
                runner.wake(&workspace, wake);
            }
        });
    }

    /// Read the change version `version` of document `id` (of `kind`) in
    /// `workspace` made, in the background, and wake who it wakes.
    pub fn changed(
        self: &Arc<Self>,
        workspace: String,
        kind: String,
        id: String,
        version: u64,
        reach: Option<Vec<String>>,
    ) {
        let runner = Arc::clone(self);
        tokio::spawn(async move {
            if let Err(error) = runner.read(&workspace, &kind, &id, version, reach).await {
                tracing::warn!(%workspace, %kind, %id, %error, "the team could not read a change");
            }
        });
    }

    async fn read(
        self: &Arc<Self>,
        workspace: &str,
        kind: &str,
        id: &str,
        version: u64,
        reach: Option<Vec<String>>,
    ) -> Result<(), ModuleError> {
        let scope = scope(None, workspace, READ_DEADLINE, &self.stop);
        let turns = self.host.turns().await?;
        let world = HostWorld::new(self.channel.clone(), &scope, workspace);
        let docs = HostDocuments::new(self.channel.clone(), &scope, workspace);
        let sessions = HostSessions::new(self.channel.clone(), &scope, workspace);
        let worker = Worker {
            docs: &docs,
            board: &world,
            library: &world,
            sessions: &sessions,
            turns: turns.as_ref(),
            workplace: &world,
            scope: &scope,
        };
        // A member woken by a write acts within the reach of whoever wrote
        // it (docs/connect.md 4a).
        for wake in worker.woken(kind, id, version).await? {
            self.wake(
                workspace,
                Wake {
                    reach: reach.clone(),
                    ..wake
                },
            );
        }
        Ok(())
    }

    /// Run `wake` now, or after the member's running turn on the task
    /// (the newest wake waits; one waiting is enough).
    fn wake(self: &Arc<Self>, workspace: &str, wake: Wake) {
        let key = (
            workspace.to_string(),
            wake.task.clone(),
            wake.member.clone(),
        );
        let stop = self.stop.child_token();
        {
            let mut running = self.running.lock();
            if let Some(run) = running.get_mut(&key) {
                run.waits = Some(wake);
                return;
            }
            running.insert(
                key.clone(),
                Run {
                    stop: stop.clone(),
                    waits: None,
                    progress: Arc::default(),
                },
            );
        }
        let runner = Arc::clone(self);
        tokio::spawn(async move { runner.run(key, wake, stop).await });
    }

    /// The member's turns on the task, until none waits.
    async fn run(self: Arc<Self>, key: Key, first: Wake, first_stop: CancellationToken) {
        let mut next = Some(first);
        let mut stop = first_stop;
        while let Some(wake) = next {
            if self.stop.is_cancelled() {
                break;
            }
            if self.take(&key.0, &wake.task) {
                self.tries(&key.0, &wake, &stop).await;
            } else {
                tracing::warn!(task = %wake.task, "the task's members took {MAX_TURNS_PER_TASK} turns: no more while the team runs");
            }
            let mut running = self.running.lock();
            next = running.get_mut(&key).and_then(|run| {
                let waits = run.waits.take();
                // A person stopped what ran: what woke it since is new work.
                if waits.is_some() && run.stop.is_cancelled() {
                    run.stop = self.stop.child_token();
                }
                stop = run.stop.clone();
                waits
            });
            if next.is_none() {
                running.remove(&key);
            }
        }
    }

    /// Count a turn on the task; whether it may be taken.
    fn take(&self, workspace: &str, task: &str) -> bool {
        let mut taken = self.taken.lock();
        let count = taken
            .entry((workspace.to_string(), task.to_string()))
            .or_insert(0);
        *count += 1;
        *count <= MAX_TURNS_PER_TASK
    }

    /// The member's work on the task, tried as the settings say; its task
    /// blocked when every try failed (or one that another would not mend),
    /// labeled for a person when a turn was cut short unfinished (the labels
    /// an earlier turn left taken off first: work that runs is neither).
    async fn tries(&self, workspace: &str, wake: &Wake, stop: &CancellationToken) {
        if let Err(error) = self
            .unlabel(
                workspace,
                &wake.task,
                &[ending::WAITING_PERSON, ending::BLOCKED],
            )
            .await
        {
            tracing::warn!(task = %wake.task, %error, "the labels of a task's earlier turn were not taken off");
        }
        if !self.attempts(workspace, wake, stop).await
            && let Err(error) = going::clear(&self.channel, &self.stop, workspace, wake).await
        {
            tracing::warn!(task = %wake.task, %error, "a task's ended work is still kept as going");
        }
    }

    /// The tries; whether the work still goes: it waits for its runner, or
    /// the team stopped under it.
    async fn attempts(&self, workspace: &str, wake: &Wake, stop: &CancellationToken) -> bool {
        let tries = ending::tries(&self.config);
        let mut last = String::new();
        let mut spent = 0;
        for tried in 1..=tries {
            if stop.is_cancelled() {
                return self.stop.is_cancelled();
            }
            spent = tried;
            let turn = self.turn(workspace, wake, stop).await;
            if stop.is_cancelled() {
                if !self.stop.is_cancelled()
                    && let Err(error) = self.stopped(workspace, wake).await
                {
                    tracing::warn!(task = %wake.task, %error, "a stopped turn was not said on its task");
                }
                return self.stop.is_cancelled();
            }
            let ended = match turn {
                Ok(end) => ending::ended(&end),
                Err(error) => ending::failed(&error),
            };
            match ended {
                Ending::Done => return false,
                Ending::Halt => {
                    if let Err(error) = self.halt(workspace, wake).await {
                        tracing::warn!(task = %wake.task, %error, "a task whose turn was cut short was not labeled");
                    }
                    return false;
                }
                Ending::Retry { why, after } => {
                    tracing::warn!(member = %wake.member, task = %wake.task, tried, %why, "a member's turn failed");
                    last = why;
                    if tried < tries {
                        tokio::select! {
                            () = tokio::time::sleep(after) => {}
                            () = stop.cancelled() => return self.stop.is_cancelled(),
                        }
                    }
                }
                Ending::Stop(why) => {
                    last = why;
                    break;
                }
                Ending::Wait(why) => {
                    if let Err(error) = self.wait(workspace, wake, &why).await {
                        tracing::warn!(task = %wake.task, %error, "a task waiting for its runner was not labeled");
                    }
                    return true;
                }
            }
        }
        if let Err(error) = self.block(workspace, wake, spent, &last).await {
            tracing::warn!(task = %wake.task, %error, "a task whose work failed was not blocked");
        }
        false
    }

    /// Label `wake`'s task blocked and say why on it, after `tries` tries.
    async fn block(
        &self,
        workspace: &str,
        wake: &Wake,
        tries: u32,
        why: &str,
    ) -> Result<(), ModuleError> {
        // The team blocks it, not the member whose work failed.
        let author = Some(crate::domain::MODULE_ID.to_string());
        let scope = scope(author, workspace, READ_DEADLINE, &self.stop);
        let world = HostWorld::new(self.channel.clone(), &scope, workspace);
        let (version, labels) = world.labels(&wake.task).await?;
        world
            .relabel(
                &wake.task,
                version,
                ending::with_label(&labels, ending::BLOCKED),
            )
            .await?;
        let said = ending::said(&wake.member, tries, why);
        crate::ports::Board::answer(&world, &wake.task, &said, None).await?;
        Ok(())
    }

    /// Keep `wake` until a runner connects; label its task waiting and say
    /// why on it, once.
    async fn wait(&self, workspace: &str, wake: &Wake, why: &str) -> Result<(), ModuleError> {
        {
            let mut waiting = self.waiting.lock();
            let known = waiting
                .iter()
                .any(|(at, w)| at == workspace && w.task == wake.task && w.member == wake.member);
            if !known {
                waiting.push((workspace.to_string(), wake.clone()));
            }
        }
        let author = Some(crate::domain::MODULE_ID.to_string());
        let scope = scope(author, workspace, READ_DEADLINE, &self.stop);
        let world = HostWorld::new(self.channel.clone(), &scope, workspace);
        let (version, labels) = world.labels(&wake.task).await?;
        if labels.iter().any(|l| l == ending::WAITING) {
            return Ok(());
        }
        world
            .relabel(
                &wake.task,
                version,
                ending::with_label(&labels, ending::WAITING),
            )
            .await?;
        let said = ending::waits(&wake.member, why);
        crate::ports::Board::answer(&world, &wake.task, &said, None).await?;
        Ok(())
    }

    /// Say on `wake`'s task that a person stopped its member's turn.
    async fn stopped(&self, workspace: &str, wake: &Wake) -> Result<(), ModuleError> {
        let author = Some(crate::domain::MODULE_ID.to_string());
        let scope = scope(author, workspace, READ_DEADLINE, &self.stop);
        let world = HostWorld::new(self.channel.clone(), &scope, workspace);
        let said = ending::stopped(&wake.member);
        crate::ports::Board::answer(&world, &wake.task, &said, None).await?;
        Ok(())
    }

    /// Label `wake`'s task as waiting for a person: its member's turn was
    /// cut short unfinished (the member said so in its comment).
    async fn halt(&self, workspace: &str, wake: &Wake) -> Result<(), ModuleError> {
        let author = Some(crate::domain::MODULE_ID.to_string());
        let scope = scope(author, workspace, READ_DEADLINE, &self.stop);
        let world = HostWorld::new(self.channel.clone(), &scope, workspace);
        let (version, labels) = world.labels(&wake.task).await?;
        if labels.iter().any(|l| l == ending::WAITING_PERSON) {
            return Ok(());
        }
        world
            .relabel(
                &wake.task,
                version,
                ending::with_label(&labels, ending::WAITING_PERSON),
            )
            .await
    }

    /// Take `off` from `task`'s labels, when it has any of them.
    async fn unlabel(&self, workspace: &str, task: &str, off: &[&str]) -> Result<(), ModuleError> {
        let author = Some(crate::domain::MODULE_ID.to_string());
        let scope = scope(author, workspace, READ_DEADLINE, &self.stop);
        let world = HostWorld::new(self.channel.clone(), &scope, workspace);
        let (version, labels) = world.labels(task).await?;
        let kept = ending::without_labels(&labels, off);
        if kept != labels {
            world.relabel(task, version, kept).await?;
        }
        Ok(())
    }

    async fn turn(
        &self,
        workspace: &str,
        wake: &Wake,
        stop: &CancellationToken,
    ) -> Result<TurnEnd, ModuleError> {
        // The turn, and every call it makes, acts within the reach of the
        // write that woke the member.
        let scope = Scope {
            reach: wake.reach.clone(),
            role: None,
            ..scope(
                Some(wake.member.clone()),
                workspace,
                ending::deadline(&self.config),
                stop,
            )
        };
        let key = (
            workspace.to_string(),
            wake.task.clone(),
            wake.member.clone(),
        );
        let progress = self
            .running
            .lock()
            .get(&key)
            .map(|run| Arc::clone(&run.progress))
            .unwrap_or_default();
        let turns = self.host.turns().await?;
        let world = HostWorld::new(self.channel.clone(), &scope, workspace);
        let docs = HostDocuments::new(self.channel.clone(), &scope, workspace);
        let sessions = HostSessions::new(self.channel.clone(), &scope, workspace);
        Worker {
            docs: &docs,
            board: &world,
            library: &world,
            sessions: &sessions,
            turns: turns.as_ref(),
            workplace: &world,
            scope: &scope,
        }
        .work(wake, &progress)
        .await
    }
}
