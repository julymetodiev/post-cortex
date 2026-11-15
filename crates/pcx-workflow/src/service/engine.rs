// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The engine (docs/modules/workflow.md, the engine): a run started from a
//! trigger, pinned to its workflow's version, and driven by one task: the
//! domain says what its journal makes next (`Plan::next`), the engine does
//! it through the ports and writes every attempt as a step before the next
//! one starts. A run goes on after a restart from its journal; a waiting
//! one goes on when its time comes, its event or decision is heard, or the
//! run it called ends.

mod act;
mod answered;
mod approve;
mod driver;
mod dry;
mod ends;
mod fire;
mod place;
mod preview;
mod settle;
mod spaces;
mod values;
mod watch;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use chrono::TimeDelta;
use parking_lot::Mutex;
use pcx_module_api::{MAX_CALL_DEPTH, ModuleError, ModuleErrorKind, Scope};
use post_cortex_proto::module::BusEvent;
use post_cortex_proto::workflow::{
    Fired, Map, Run, RunStatus, Step, StepStatus, Workflow, trigger,
};
use tokio::sync::{Notify, broadcast};
use tokio_util::sync::CancellationToken;

use crate::domain::{self, Catalog, failure, grants, kinds, open};
use crate::ports::{Blobs, Clock, Dry, Events, Kept, Models, Runs, State, Tools, Turns, Workflows};
use crate::service::Service;

pub use spaces::Spaces;
pub use values::{run_input, written, written_map};
pub use watch::{Watched, Written};

/// A run's steps at most, by default.
const STEPS: u32 = 1_000;

/// A run's wall time, by default.
const WALL: TimeDelta = TimeDelta::hours(24);

/// Runs of one workflow failed in a row that make it `failing`.
const FAILING: usize = 3;

/// Runs a listing gives when it names no limit.
const RUNS: usize = 50;

/// The longest the driver sleeps without looking again.
const IDLE: Duration = Duration::from_secs(60);

/// The events the module emits.
pub mod events {
    /// A run succeeded.
    pub const SUCCEEDED: &str = "workflow.run_succeeded";
    /// A run failed.
    pub const FAILED: &str = "workflow.run_failed";
    /// Three runs of a workflow failed in a row.
    pub const FAILING: &str = "workflow.failing";
}

/// What the engine reaches the world through.
pub struct Ports {
    /// The workflows.
    pub workflows: Arc<dyn Workflows>,
    /// The runs and their journals.
    pub runs: Arc<dyn Runs>,
    /// The workflows' stores.
    pub state: Arc<dyn State>,
    /// The tools.
    pub tools: Arc<dyn Tools>,
    /// The models.
    pub models: Arc<dyn Models>,
    /// The agent's turns.
    pub turns: Arc<dyn Turns>,
    /// The bus.
    pub events: Arc<dyn Events>,
    /// Memory's blobs.
    pub blobs: Arc<dyn Blobs>,
    /// The time.
    pub clock: Arc<dyn Clock>,
    /// What a dry run stores in.
    pub dry: Arc<dyn Dry>,
}

/// The module's settings the engine reads.
#[derive(Debug, Clone, Default)]
pub struct Settings {
    /// The workspace its runs act in.
    pub workspace: String,
    /// The board a run's task and a workflow's approval are added to;
    /// empty: none.
    pub board: String,
    /// A workspace other than the settings': its approvals go to that
    /// workspace's own board, not `board` (`Engine::approvals`).
    pub own_board: bool,
    /// A dry run's engine: what it does for real; none: not a dry run.
    pub dry: Option<DryRun>,
}

/// What a dry run does for real (docs/modules/workflow.md, triggers and
/// approval).
#[derive(Debug, Clone, Copy, Default)]
pub struct DryRun {
    /// Its `model` and `agent` blocks.
    pub models: bool,
}

/// How a run is started: which of its triggers.
#[derive(Debug, Clone)]
pub enum Start {
    /// `workflow_run` by a person or an agent.
    Manual,
    /// A `call` block of another workflow, or a tool call.
    Call,
    /// A job of the scheduler.
    Schedule,
    /// A request of a webhook source.
    Webhook(String),
    /// An event of the bus, by its trigger block.
    Event(String),
    /// A dry run: its first trigger, whichever it is.
    Dry,
}

impl Start {
    fn fits(&self, block: &str, kind: &trigger::Kind) -> bool {
        match (self, kind) {
            (Self::Manual, trigger::Kind::Manual(_))
            | (Self::Call, trigger::Kind::Call(_))
            | (Self::Schedule, trigger::Kind::Schedule(_)) => true,
            (Self::Webhook(source), trigger::Kind::Webhook(w)) => &w.source == source,
            (Self::Event(trigger), trigger::Kind::Event(_)) => trigger == block,
            (Self::Dry, _) => true,
            _ => false,
        }
    }
}

/// What a run is started with.
#[derive(Debug, Clone)]
pub enum Input {
    /// Its input, by the input's fields.
    Map(Map),
    /// A webhook's body: the fields the input names.
    Body(serde_json::Value),
}

/// An `event` trigger of a workflow: what the engine listens for.
#[derive(Debug, Clone)]
pub struct Listener {
    /// The workflow's workspace: only its events fire the trigger.
    pub workspace: String,
    /// The workflow.
    pub workflow: String,
    /// Its trigger block.
    pub block: String,
    /// The trigger; its filter is compiled when an event of its name comes
    /// (`Engine::fire`), with what the workflow reaches then.
    pub trigger: post_cortex_proto::workflow::EventTrigger,
}

/// A run being driven.
pub struct Live {
    wake: Arc<Notify>,
    cancel: CancellationToken,
    /// Events heard for it, not yet read.
    inbox: Mutex<Vec<BusEvent>>,
    /// The events its waiting steps listen for.
    listens: Mutex<HashSet<String>>,
}

/// The engine of the module's runs, at work in one workspace: its ports and
/// settings are that workspace's, everything else is the one engine's,
/// shared with it in every other workspace ([`Engine::at`]).
pub struct Engine {
    ports: Ports,
    settings: Settings,
    /// Its ports and settings in the other workspaces; none: it works in
    /// one workspace only.
    spaces: Option<Arc<dyn Spaces>>,
    /// The module's own messages: the board's, the bus's, its own.
    own: Arc<Catalog>,
    /// Each workspace's own board of approvals, once found or made.
    boards: Arc<tokio::sync::Mutex<HashMap<String, String>>>,
    /// The runs being driven, of every workspace, by workspace and id
    /// (`Engine::live_key`).
    live: Arc<Mutex<Driven>>,
    /// The `event` triggers of every workspace's workflows.
    listeners: Arc<Mutex<Vec<Listener>>>,
    /// Cancelled when the module stops: the drivers stop, the runs stay
    /// open and go on after the next start.
    stopping: CancellationToken,
    /// What it writes of runs, for their watchers.
    watched: broadcast::Sender<Written>,
}

/// The runs being driven, each by its workspace and id.
type Driven = HashMap<(String, String), Arc<Live>>;

/// A step by its graph, block and attempt.
pub type Key = (String, String, u32);

fn key_of(step: &Step) -> Key {
    (step.at.clone(), step.block.clone(), step.attempt)
}

fn now_ts(clock: &dyn Clock) -> prost_types::Timestamp {
    domain::timestamp(clock.now())
}

impl Engine {
    /// The engine over `ports` with `settings`.
    ///
    /// # Errors
    /// `Internal` when the module's descriptors do not read.
    pub fn new(ports: Ports, settings: Settings) -> Result<Arc<Self>, ModuleError> {
        Self::built(ports, settings, None)
    }

    fn built(
        ports: Ports,
        settings: Settings,
        spaces: Option<Arc<dyn Spaces>>,
    ) -> Result<Arc<Self>, ModuleError> {
        let own = Catalog::new(Vec::new())
            .map_err(|why| ModuleError::new(ModuleErrorKind::Internal, why))?;
        Ok(Arc::new(Self {
            ports,
            settings,
            spaces,
            own: Arc::new(own),
            boards: Arc::default(),
            live: Arc::default(),
            listeners: Arc::default(),
            stopping: CancellationToken::new(),
            watched: broadcast::channel(watch::BACKLOG).0,
        }))
    }

    /// Stop driving the runs: they stay as their journals have them.
    pub fn stop(&self) {
        self.stopping.cancel();
    }

    fn service(&self) -> Service<'_> {
        Service::new(self.ports.workflows.as_ref(), self.ports.tools.as_ref())
    }

    /// Whether this engine runs dry.
    fn dry(&self) -> bool {
        self.settings.dry.is_some()
    }

    /// The scope a run of `workflow` acts in: its author `workflow:<name>`,
    /// its workspace, `depth` calls deep.
    fn scope(&self, workflow: &str, depth: u32, cancel: CancellationToken) -> Scope {
        let wire = post_cortex_proto::module::Scope {
            workspace_id: self.settings.workspace.clone(),
            author: format!("workflow:{workflow}"),
            call_depth: depth,
            ..Default::default()
        };
        Scope::from_wire(wire, cancel, Duration::from_secs(3_600))
    }

    /// Start a run of workflow `name` by its trigger `start`, with `input`
    /// and what fired it, `parent` the run whose `call` block starts it.
    ///
    /// # Errors
    /// `NotFound` for no such workflow; `FailedPrecondition` for one with
    /// no such trigger or whose grants are not approved; `InvalidArgument`
    /// for an input that does not read or a workflow that does not check;
    /// `Denied` past the host's depth; memory failed.
    pub async fn start(
        self: &Arc<Self>,
        name: &str,
        start: &Start,
        fired: Fired,
        input: &Input,
        parent: Option<&Kept<Run>>,
    ) -> Result<Kept<Run>, ModuleError> {
        let kept = self.open_run(name, start, fired, input, parent).await?;
        self.drive(kept.id.clone());
        Ok(kept)
    }

    /// A run of workflow `name` written as [`Engine::start`] starts it, not
    /// yet driven.
    ///
    /// # Errors
    /// As [`Engine::start`]'s.
    async fn open_run(
        &self,
        name: &str,
        start: &Start,
        fired: Fired,
        input: &Input,
        parent: Option<&Kept<Run>>,
    ) -> Result<Kept<Run>, ModuleError> {
        let depth = parent.map_or(0, |p| p.data.call_depth + 1);
        if depth > MAX_CALL_DEPTH {
            return Err(ModuleError::new(
                ModuleErrorKind::Denied,
                format!("{name} would run {depth} calls deep; the limit is {MAX_CALL_DEPTH}"),
            ));
        }
        let workflow = self.ports.workflows.named(name).await?.ok_or_else(|| {
            ModuleError::new(ModuleErrorKind::NotFound, format!("no workflow {name}"))
        })?;
        let trigger = trigger_of(&workflow.data, start).ok_or_else(|| {
            ModuleError::new(
                ModuleErrorKind::FailedPrecondition,
                format!("{name} has no trigger that starts it this way"),
            )
        })?;
        let scope = self.scope(name, depth, CancellationToken::new());
        let (catalog, checked) = self.service().checked(&scope, &workflow.data).await?;
        // Judged on the tools its graph calls as this run reaches them, not
        // on those it was kept with.
        let calling = grants::calling(workflow.data.clone(), &checked.found);
        if !self.dry() && !grants::approved(&calling) {
            let board = self.approvals(false).await?.unwrap_or_default();
            return Err(ModuleError::new(
                ModuleErrorKind::FailedPrecondition,
                format!(
                    "{name} waits for a person to approve its grants (its task on the board {board})"
                ),
            ));
        }
        let read = match input {
            Input::Map(map) => {
                domain::read_input(&checked.signature.input, map, catalog.messages())
            }
            Input::Body(json) => pcx_expr::schema::read_body(&checked.signature.input, json),
        }
        .map_err(|why| {
            ModuleError::new(ModuleErrorKind::InvalidArgument, format!("input: {why}"))
                .with_field("input")
        })?;
        let now = now_ts(self.ports.clock.as_ref());
        let run = Run {
            workflow: name.to_string(),
            workflow_id: workflow.id.clone(),
            version: workflow.version,
            trigger: Some(Fired {
                block: trigger.clone(),
                at: Some(now),
                ..fired
            }),
            input: Some(written_map(&read)?),
            status: RunStatus::Running.into(),
            started_at: Some(now),
            call_depth: depth,
            parent: parent.map(|p| p.id.clone()).unwrap_or_default(),
            ..Default::default()
        };
        let kept = self.ports.runs.create(&run).await?;
        self.wrote(Written::Run(Box::new(kept.clone())));
        let started = self
            .ports
            .runs
            .put_step(&Step {
                run: kept.id.clone(),
                block: trigger,
                attempt: 1,
                status: StepStatus::Succeeded.into(),
                port: "next".into(),
                started_at: Some(now),
                ended_at: Some(now),
                ..Default::default()
            })
            .await?;
        self.wrote(Written::Step(Box::new(started.data)));
        Ok(kept)
    }

    /// After the module starts: the `event` triggers read, the decisions
    /// on grants approved meanwhile read again, what the runs that ended
    /// last left waiting settled, and every run that has not ended driven.
    ///
    /// # Errors
    /// Memory failed.
    pub async fn resume(self: &Arc<Self>) -> Result<usize, ModuleError> {
        self.listen().await?;
        self.recheck().await?;
        self.sweep().await?;
        let open = self.ports.runs.open().await?;
        for run in &open {
            self.drive(run.id.clone());
        }
        Ok(open.len())
    }

    /// Cancel run `id`.
    ///
    /// # Errors
    /// `NotFound`; `FailedPrecondition` for a run that ended; memory
    /// failed.
    pub async fn cancel(self: &Arc<Self>, id: &str) -> Result<Kept<Run>, ModuleError> {
        let run =
            self.ports.runs.get(id).await?.ok_or_else(|| {
                ModuleError::new(ModuleErrorKind::NotFound, format!("no run {id}"))
            })?;
        if !open(&run.data) {
            return Err(ModuleError::new(
                ModuleErrorKind::FailedPrecondition,
                format!("run {id} has ended"),
            ));
        }
        let live = self.live.lock().get(&self.live_key(id)).cloned();
        match live {
            // Its driver ends it.
            Some(live) => {
                live.cancel.cancel();
                Ok(run)
            }
            None => {
                self.end(
                    &run,
                    RunStatus::Cancelled,
                    None,
                    Some(failure(kinds::CANCELLED, "cancelled")),
                )
                .await
            }
        }
    }

    /// The runs of workflow `workflow`, the most recently changed first, at
    /// most `limit` (0: 50).
    ///
    /// # Errors
    /// Memory failed.
    pub async fn runs(&self, workflow: &str, limit: u32) -> Result<Vec<Kept<Run>>, ModuleError> {
        // A u32 fits a usize here.
        let limit = if limit == 0 { RUNS } else { limit as usize };
        self.ports.runs.of(workflow, limit).await
    }

    /// The runs, for what lists them with their workflows.
    #[must_use]
    pub fn runs_port(&self) -> &dyn Runs {
        self.ports.runs.as_ref()
    }
}

/// The trigger block of `workflow` that `start` starts.
fn trigger_of(workflow: &Workflow, start: &Start) -> Option<String> {
    workflow
        .graph
        .as_ref()?
        .blocks
        .iter()
        .find_map(|b| match &b.kind {
            Some(post_cortex_proto::workflow::block::Kind::Trigger(t)) => t
                .kind
                .as_ref()
                .filter(|k| start.fits(&b.id, k))
                .map(|_| b.id.clone()),
            _ => None,
        })
}

#[cfg(test)]
mod approval_tests;
#[cfg(test)]
mod data_tests;
#[cfg(test)]
mod grant_tests;
#[cfg(test)]
mod space_tests;
#[cfg(test)]
mod test_engine;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod turn_tests;
