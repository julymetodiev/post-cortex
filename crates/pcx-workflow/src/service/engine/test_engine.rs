// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! An engine over documents held in memory, a board that answers in
//! memory and ports that are not asked: what the engine's tests run on.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use parking_lot::Mutex;
use pcx_documents::InMemoryDocuments;
use pcx_module_api::bus::EventFeed;
use pcx_module_api::{ModuleError, ModuleErrorKind, Scope};
use post_cortex_proto::board::{
    Decision, DecisionApproved, DecisionRecord, Task, TaskList, TaskRecord, Thread,
};
use post_cortex_proto::module::{BusEvent, WorkflowTemplate};
use post_cortex_proto::workflow::{Approved, Map, Run, RunStatus, Value as Proto, Workflow, value};
use prost::{Message, Name};
use prost_reflect::DynamicMessage;
use serde_json::Value as Json;
use tokio_util::sync::CancellationToken;

use crate::adapters::{DocumentRuns, DocumentWorkflows, DryStores, SystemClock};
use crate::domain::Tool;
use crate::domain::open;
use crate::ports::{Answer, Blobs, Events, Kept, Models, Said, Tools, Turns, Whose, Workflows};
use crate::service::engine::{Engine, Ports, Settings, Spaces};
use crate::test_support::{ADD, DECIDE, GET, LIST, WITHDRAW, board_tools, catalog};

/// The board, answering in memory: `tasks` tasks; a decision that waits,
/// or that its policy approves (`policy`); the decisions approved meanwhile
/// (`approved`), as `GetTask` reads them.
pub struct Board {
    pub tasks: usize,
    pub policy: bool,
    pub approved: Mutex<Option<String>>,
    pub calls: Mutex<Vec<(String, Option<String>)>>,
    /// The decisions asked, as they were asked.
    pub decided: Mutex<Vec<DynamicMessage>>,
    /// Whether it refuses a decision (an author it does not admit).
    pub refuses: Mutex<bool>,
    /// The tools of other modules reached beside the board's.
    pub others: Mutex<Vec<(String, Tool)>>,
    /// The workflow templates the modules reached ship.
    pub templates: Mutex<Vec<(String, WorkflowTemplate)>>,
}

impl Board {
    /// The paths called, in order.
    pub fn paths(&self) -> Vec<String> {
        self.calls.lock().iter().map(|(p, _)| p.clone()).collect()
    }
}

fn answer(message: &impl Message, name: &str) -> DynamicMessage {
    let pool = catalog().messages().clone();
    DynamicMessage::decode(
        pool.get_message_by_name(name).unwrap(),
        message.encode_to_vec().as_slice(),
    )
    .unwrap()
}

#[async_trait]
impl Tools for Board {
    async fn reachable(&self, _scope: &Scope) -> Result<Vec<(String, Tool)>, ModuleError> {
        let mut tools = board_tools();
        tools.extend(self.others.lock().iter().cloned());
        Ok(tools)
    }

    async fn call(
        &self,
        _scope: &Scope,
        path: &str,
        request: &DynamicMessage,
        key: Option<&str>,
    ) -> Result<DynamicMessage, ModuleError> {
        self.calls
            .lock()
            .push((path.to_string(), key.map(str::to_string)));
        if path == DECIDE {
            self.decided.lock().push(request.clone());
        }
        let decision = |approved_by: Option<String>| DecisionRecord {
            id: "decision-1".into(),
            pending: approved_by.is_none(),
            decision: Some(Decision {
                task: "task-1".into(),
                what: "Add?".into(),
                approved_by,
                ..Default::default()
            }),
            ..Default::default()
        };
        Ok(match path {
            LIST => answer(
                &TaskList {
                    tasks: (0..self.tasks)
                        .map(|i| TaskRecord {
                            id: format!("t{i}"),
                            task: Some(Task::default()),
                            ..Default::default()
                        })
                        .collect(),
                    ..Default::default()
                },
                "pcx.board.v1.TaskList",
            ),
            ADD => answer(
                &TaskRecord {
                    id: "task-1".into(),
                    ..Default::default()
                },
                "pcx.board.v1.TaskRecord",
            ),
            DECIDE if *self.refuses.lock() => {
                return Err(ModuleError::new(
                    ModuleErrorKind::Denied,
                    "the author is not a member of the workspace's teams",
                ));
            }
            DECIDE => answer(
                &decision(self.policy.then(|| "policy:1".to_string())),
                "pcx.board.v1.DecisionRecord",
            ),
            GET => answer(
                &Thread {
                    decisions: vec![decision(self.approved.lock().clone())],
                    ..Default::default()
                },
                "pcx.board.v1.Thread",
            ),
            WITHDRAW => {
                let mut withdrawn = decision(None);
                withdrawn.pending = false;
                if let Some(d) = withdrawn.decision.as_mut() {
                    d.withdrawn = Some("its run ended".into());
                }
                answer(&withdrawn, "pcx.board.v1.DecisionRecord")
            }
            other => {
                return Err(ModuleError::new(
                    ModuleErrorKind::NotFound,
                    other.to_string(),
                ));
            }
        })
    }

    async fn templates(
        &self,
        _scope: &Scope,
    ) -> Result<Vec<(String, WorkflowTemplate)>, ModuleError> {
        Ok(self.templates.lock().clone())
    }
}

/// Nothing of these is asked here; the events published are kept, and a
/// turn of an absent member cannot be asked.
#[derive(Default)]
pub struct Unused {
    pub published: Mutex<Vec<String>>,
    /// The members no team has: a turn of one cannot be asked.
    pub absent: Mutex<Vec<String>>,
    /// What the agent's next turns answer, in order; none: no agent here.
    pub says: Mutex<std::collections::VecDeque<String>>,
    /// The prompts its turns were asked.
    pub asked: Mutex<Vec<String>>,
}

#[async_trait]
impl Models for Unused {
    async fn answer(
        &self,
        _: &Scope,
        _: &str,
        _: &str,
        _: &str,
        _: &Json,
    ) -> Result<Answer, ModuleError> {
        Err(ModuleError::new(
            ModuleErrorKind::Unavailable,
            "no models here",
        ))
    }
}

#[async_trait]
impl Turns for Unused {
    async fn turn(
        &self,
        _: &Scope,
        _: &str,
        _: Whose<'_>,
        prompt: &str,
    ) -> Result<Said, ModuleError> {
        self.asked.lock().push(prompt.to_string());
        match self.says.lock().pop_front() {
            Some(answer) => Ok(Said { answer, tokens: 1 }),
            None => Err(ModuleError::new(
                ModuleErrorKind::Unavailable,
                "no agent here",
            )),
        }
    }

    async fn ready(&self, whose: Whose<'_>) -> Result<(), ModuleError> {
        match whose {
            Whose::Member(member) if self.absent.lock().iter().any(|m| m == member) => {
                Err(ModuleError::new(
                    ModuleErrorKind::FailedPrecondition,
                    format!("no member {member} in the workspace's teams"),
                ))
            }
            _ => Ok(()),
        }
    }

    async fn session(&self, _: &str) -> Result<String, ModuleError> {
        Ok("session-1".into())
    }
}

#[async_trait]
impl Events for Unused {
    async fn publish(
        &self,
        _: &Scope,
        name: &str,
        _: pbjson_types::Any,
    ) -> Result<(), ModuleError> {
        self.published.lock().push(name.to_string());
        Ok(())
    }

    async fn follow(&self) -> Result<EventFeed, ModuleError> {
        Err(ModuleError::new(
            ModuleErrorKind::FailedPrecondition,
            "no events here",
        ))
    }

    async fn ack(&self, _: &str, _: u64) -> Result<(), ModuleError> {
        Ok(())
    }
}

#[async_trait]
impl Blobs for Unused {
    async fn put(&self, _: Vec<u8>) -> Result<String, ModuleError> {
        Err(ModuleError::new(
            ModuleErrorKind::Unavailable,
            "no blobs here",
        ))
    }

    async fn get(&self, _: &str) -> Result<Vec<u8>, ModuleError> {
        Err(ModuleError::new(
            ModuleErrorKind::Unavailable,
            "no blobs here",
        ))
    }
}

/// The engine's ports over `docs` and `board`, its events published into
/// `events`.
fn ports(docs: &Arc<InMemoryDocuments>, board: Arc<Board>, events: Arc<Unused>) -> Ports {
    let runs = Arc::new(DocumentRuns::new(docs.clone()));
    let workflows = Arc::new(DocumentWorkflows::new(docs.clone()));
    let unused = Arc::new(Unused::default());
    let dry = DryStores {
        workflows: workflows.clone(),
        state: runs.clone(),
        blobs: unused.clone(),
    };
    Ports {
        workflows,
        runs: runs.clone(),
        state: runs,
        tools: board,
        models: unused.clone(),
        turns: events.clone(),
        events,
        blobs: unused,
        clock: Arc::new(SystemClock),
        dry: Arc::new(dry),
    }
}

fn settings(workspace: &str) -> Settings {
    Settings {
        workspace: workspace.into(),
        board: "b1".into(),
        own_board: false,
        dry: None,
    }
}

/// An engine over `docs` and `board`, its events published into `events`.
pub fn engine_with(
    docs: &Arc<InMemoryDocuments>,
    board: Arc<Board>,
    events: Arc<Unused>,
) -> Arc<Engine> {
    Engine::new(ports(docs, board, events), settings("ws")).unwrap()
}

/// Workspaces, each with its own documents, over one board.
pub struct Workspaces {
    pub docs: HashMap<String, Arc<InMemoryDocuments>>,
    pub board: Arc<Board>,
}

impl Spaces for Workspaces {
    fn of(&self, workspace: &str) -> (Ports, Settings) {
        let docs = &self.docs[workspace];
        (
            ports(docs, self.board.clone(), Arc::new(Unused::default())),
            settings(workspace),
        )
    }
}

/// An engine over `docs` and `board`.
pub fn engine(docs: &Arc<InMemoryDocuments>, board: Arc<Board>) -> Arc<Engine> {
    engine_with(docs, board, Arc::new(Unused::default()))
}

pub fn docs() -> Arc<InMemoryDocuments> {
    Arc::new(
        InMemoryDocuments::new(Some("ana"), post_cortex_proto::WORKFLOW_DESCRIPTOR_SET).unwrap(),
    )
}

pub fn board(tasks: usize) -> Arc<Board> {
    Arc::new(Board {
        tasks,
        policy: false,
        approved: Mutex::new(None),
        calls: Mutex::new(Vec::new()),
        decided: Mutex::new(Vec::new()),
        refuses: Mutex::new(false),
        others: Mutex::new(Vec::new()),
        templates: Mutex::new(Vec::new()),
    })
}

pub fn input(limit: i64) -> Map {
    let v = |kind| Proto { kind: Some(kind) };
    Map {
        entries: [
            ("board".to_string(), v(value::Kind::String("b1".into()))),
            ("limit".to_string(), v(value::Kind::Int(limit))),
        ]
        .into(),
    }
}

/// The board's event of decision `decision-1` approved by `by`.
pub fn approved(by: &str) -> BusEvent {
    BusEvent {
        name: "board.decision_approved".into(),
        payload: Some(pbjson_types::Any {
            type_url: DecisionApproved::type_url(),
            value: DecisionApproved {
                decision: "decision-1".into(),
                task: "task-1".into(),
                approved_by: by.into(),
            }
            .encode_to_vec()
            .into(),
        }),
        ..Default::default()
    }
}

/// Run `id` once it has ended (or waits, when `waiting`).
pub async fn settled(engine: &Engine, id: &str, waiting: bool) -> Kept<Run> {
    for _ in 0..200 {
        let run = engine.latest(id).await.unwrap();
        let status = RunStatus::try_from(run.data.status).unwrap();
        if !open(&run.data) || (waiting && status == RunStatus::Waiting) {
            return run;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let run = engine.latest(id).await.unwrap();
    let (_, steps) = engine.detail(id).await.unwrap();
    let steps: Vec<_> = steps
        .into_iter()
        .map(|s| (s.block, s.status, s.waits_for, s.failure))
        .collect();
    panic!(
        "run {id} did not settle: {:?} {:?} {steps:?}",
        run.data.status, run.data.failure
    );
}

/// `w` kept in `docs`, its grants approved.
pub async fn kept(docs: &Arc<InMemoryDocuments>, w: &Workflow) -> Kept<Workflow> {
    let approved = Workflow {
        approved: Some(Approved {
            grants: w.grants.clone(),
            decision: "d-0".into(),
            approved_by: "ana".into(),
        }),
        ..w.clone()
    };
    DocumentWorkflows::new(docs.clone())
        .create(&approved)
        .await
        .unwrap()
}

/// A scope of `author`, called from `module` (none: not a module).
pub fn scope_of(author: &str, module: Option<&str>) -> Scope {
    let wire = post_cortex_proto::module::Scope {
        workspace_id: "ws".into(),
        author: author.into(),
        caller_module: module.unwrap_or_default().into(),
        ..Default::default()
    };
    Scope::from_wire(wire, CancellationToken::new(), Duration::from_secs(60))
}

/// Workflow `name` as the engine's workflows keep it.
pub async fn read(engine: &Engine, name: &str) -> Workflow {
    engine
        .ports
        .workflows
        .named(name)
        .await
        .unwrap()
        .unwrap()
        .data
}
