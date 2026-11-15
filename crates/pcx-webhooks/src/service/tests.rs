// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The webhooks' service over a board and memory held in memory.

use async_trait::async_trait;
use parking_lot::Mutex;

use super::*;

use crate::alertmanager::tests::FIRING;
use crate::domain::{Incident, NO_ACTION};
use crate::ports::{Board as BoardPort, Incidents};
use post_cortex_proto::board::{
    AddTaskRequest, Board as Layout, BoardRecord, CommentRecord, Stage, Task, Thread,
};

/// A board with To do, Done and Canceled, and what was done to it.
#[derive(Default)]
struct Memory {
    tasks: Mutex<Vec<TaskRecord>>,
    comments: Mutex<Vec<(String, String)>>,
    incidents: Mutex<Vec<(String, Incident)>>,
    told: Mutex<Vec<(String, String)>>,
    /// Who closed each task.
    closed_by: Mutex<Vec<String>>,
    /// The member the board refuses to let close, if any.
    refuses: Mutex<Option<String>>,
    /// The workflows run: the workflow, the source, the body.
    runs: Mutex<Vec<(String, String, Vec<u8>)>>,
}

impl Memory {
    fn task(&self, i: usize) -> TaskRecord {
        self.tasks.lock()[i].clone()
    }

    fn set(&self, i: usize, change: impl FnOnce(&mut TaskRecord)) {
        change(&mut self.tasks.lock()[i]);
    }
}

#[async_trait]
impl BoardPort for Memory {
    async fn add(&self, task: AddTaskRequest) -> Result<TaskRecord, ModuleError> {
        let mut tasks = self.tasks.lock();
        let record = TaskRecord {
            id: format!("t-{}", tasks.len() + 1),
            version: 1,
            task: Some(Task {
                title: task.title,
                board: task.board,
                column: "To do".into(),
                assignee: task.assignee,
                labels: task.labels,
                key: task.key.unwrap_or_default(),
                ..Task::default()
            }),
            ..TaskRecord::default()
        };
        tasks.push(record.clone());
        Ok(record)
    }

    async fn tasks(&self, _board: &str) -> Result<Vec<TaskRecord>, ModuleError> {
        Ok(self.tasks.lock().clone())
    }

    async fn board(&self, board: &str) -> Result<BoardRecord, ModuleError> {
        Ok(BoardRecord {
            id: board.to_string(),
            board: Some(Layout {
                name: "Ops".into(),
                columns: vec!["To do".into(), "Done".into(), "Canceled".into()],
                stages: vec![
                    Stage::Open.into(),
                    Stage::Done.into(),
                    Stage::Canceled.into(),
                ],
                session_id: "s-ops".into(),
                ..Layout::default()
            }),
            ..BoardRecord::default()
        })
    }

    async fn thread(&self, task: &str) -> Result<Thread, ModuleError> {
        let comments = self
            .comments
            .lock()
            .iter()
            .filter(|(t, _)| t == task)
            .map(|(_, text)| CommentRecord {
                text: text.clone(),
                author: Some(
                    if text.starts_with("Disk") {
                        "eq-sentinel"
                    } else {
                        "alertmanager"
                    }
                    .into(),
                ),
                ..CommentRecord::default()
            })
            .collect();
        Ok(Thread {
            comments,
            ..Thread::default()
        })
    }

    async fn comment(&self, task: &str, text: &str) -> Result<(), ModuleError> {
        self.comments
            .lock()
            .push((task.to_string(), text.to_string()));
        Ok(())
    }

    async fn close(
        &self,
        task: &str,
        version: u64,
        column: &str,
        member: &str,
    ) -> Result<(), ModuleError> {
        if self.refuses.lock().as_deref() == Some(member) {
            return Err(ModuleError::new(
                pcx_module_api::ModuleErrorKind::Denied,
                format!("{member} may not close"),
            ));
        }
        self.closed_by.lock().push(member.to_string());
        let mut tasks = self.tasks.lock();
        let record = tasks.iter_mut().find(|t| t.id == task).unwrap();
        assert_eq!(record.version, version, "closed from the version read");
        record.version += 1;
        record.task.as_mut().unwrap().column = column.to_string();
        Ok(())
    }
}

#[async_trait]
impl Incidents for Memory {
    async fn record(&self, session: &str, incident: &Incident) -> Result<(), ModuleError> {
        self.incidents
            .lock()
            .push((session.to_string(), incident.clone()));
        Ok(())
    }
}

impl World for Memory {
    fn of<'a>(&'a self, _source: &'a Source) -> Box<dyn Reached + 'a> {
        Box::new(self)
    }
}

#[async_trait]
impl BoardPort for &Memory {
    async fn add(&self, task: AddTaskRequest) -> Result<TaskRecord, ModuleError> {
        (*self).add(task).await
    }
    async fn tasks(&self, board: &str) -> Result<Vec<TaskRecord>, ModuleError> {
        BoardPort::tasks(*self, board).await
    }
    async fn board(&self, board: &str) -> Result<BoardRecord, ModuleError> {
        (*self).board(board).await
    }
    async fn thread(&self, task: &str) -> Result<Thread, ModuleError> {
        (*self).thread(task).await
    }
    async fn comment(&self, task: &str, text: &str) -> Result<(), ModuleError> {
        (*self).comment(task, text).await
    }
    async fn close(
        &self,
        task: &str,
        version: u64,
        column: &str,
        member: &str,
    ) -> Result<(), ModuleError> {
        (*self).close(task, version, column, member).await
    }
}

#[async_trait]
impl crate::ports::Teller for Memory {
    async fn tell(&self, destination: &str, text: &str) -> Result<(), ModuleError> {
        self.told
            .lock()
            .push((destination.to_string(), text.to_string()));
        Ok(())
    }
}

#[async_trait]
impl crate::ports::Teller for &Memory {
    async fn tell(&self, destination: &str, text: &str) -> Result<(), ModuleError> {
        crate::ports::Teller::tell(*self, destination, text).await
    }
}

#[async_trait]
impl crate::ports::Runner for &Memory {
    async fn run(
        &self,
        workflow: &str,
        source: &str,
        body: Vec<u8>,
    ) -> Result<String, ModuleError> {
        let mut runs = self.runs.lock();
        runs.push((workflow.to_string(), source.to_string(), body));
        Ok(format!("r-{}", runs.len()))
    }
}

#[async_trait]
impl Incidents for &Memory {
    async fn record(&self, session: &str, incident: &Incident) -> Result<(), ModuleError> {
        (*self).record(session, incident).await
    }
}

fn config() -> WebhooksConfig {
    WebhooksConfig {
        token: "s3cret".into(),
        sources: vec![Source {
            name: "alertmanager".into(),
            kind: SourceKind::Alertmanager.into(),
            workspace_id: "ws".into(),
            board: "b".into(),
            assignee: "eq-sentinel".into(),
            notify: "ops".into(),
            workflow: String::new(),
            heartbeats: vec!["Watchdog".into()],
        }],
    }
}

fn request(body: &str) -> ReceiveRequest {
    ReceiveRequest {
        source: "alertmanager".into(),
        token: "s3cret".into(),
        body: body.as_bytes().to_vec(),
    }
}

/// The firing body, resolved (its alert ended 10 minutes after it began).
fn resolved() -> String {
    FIRING
        .replace(r#""status": "firing""#, r#""status": "resolved""#)
        .replace("0001-01-01T00:00:00Z", "2026-10-27T07:30:11.512Z")
}

#[tokio::test]
async fn an_incident_is_one_task_from_firing_to_resolved() {
    let (config, memory) = (config(), Memory::default());
    let service = Service::new(&config, &memory);
    let added = service.receive(request(FIRING)).await.unwrap();
    assert_eq!(added.handled(), Handled::Added);
    assert!(added.told);
    assert_eq!(
        memory.told.lock().clone(),
        [(
            "ops".to_string(),
            "warning: PodCrashLooping · equal-rust\nsvc-music restarts".to_string()
        )]
    );
    let task = memory.task(0);
    assert_eq!(added.task, task.id);
    assert_eq!(
        task.task.as_ref().unwrap().assignee.as_deref(),
        Some("eq-sentinel")
    );
    assert!(task.task.as_ref().unwrap().key.ends_with(":1"));

    let again = service.receive(request(FIRING)).await.unwrap();
    assert_eq!(again.handled(), Handled::FiringAgain);
    assert_eq!(
        memory.told.lock().len(),
        1,
        "told once, of the new incident"
    );
    assert_eq!(again.task, task.id);
    assert_eq!(memory.tasks.lock().len(), 1, "no second task");

    memory
        .comment(
            &task.id,
            "Disk is fine: a restart loop, fixed by the deploy",
        )
        .await
        .unwrap();
    let done = service.receive(request(&resolved())).await.unwrap();
    assert_eq!(done.handled(), Handled::Resolved, "it stays for its work");
    assert_eq!(memory.task(0).task.unwrap().column, "To do");
    let comments = memory.comments.lock().clone();
    assert_eq!(comments.last().unwrap().1, "Resolved after 10 min.");
    let incidents = memory.incidents.lock().clone();
    assert_eq!(incidents.len(), 1);
    assert_eq!(incidents[0].0, "s-ops", "in the board's session");
    assert!(
        incidents[0]
            .1
            .description
            .contains("eq-sentinel: Disk is fine")
    );
    assert!(
        !incidents[0].1.description.contains("Fires again"),
        "not the source's own comments"
    );
}

#[tokio::test]
async fn a_task_found_to_need_nothing_closes_and_the_next_incident_is_a_new_task() {
    let (config, memory) = (config(), Memory::default());
    let service = Service::new(&config, &memory);
    service.receive(request(FIRING)).await.unwrap();
    memory.set(0, |t| {
        t.task.as_mut().unwrap().labels.push(NO_ACTION.into())
    });
    let closed = service.receive(request(&resolved())).await.unwrap();
    assert_eq!(closed.handled(), Handled::Closed);
    assert_eq!(memory.task(0).task.unwrap().column, "Done");
    let member = memory.task(0).task.unwrap().assignee.unwrap();
    assert_eq!(
        *memory.closed_by.lock(),
        [member],
        "closed as its member, not the source"
    );

    let next = service.receive(request(FIRING)).await.unwrap();
    assert_eq!(next.handled(), Handled::Added);
    assert_eq!(memory.tasks.lock().len(), 2);
    assert!(memory.task(1).task.unwrap().key.ends_with(":2"));
}

#[tokio::test]
async fn a_held_task_stays_open_when_its_group_resolves() {
    let (config, memory) = (config(), Memory::default());
    let service = Service::new(&config, &memory);
    service.receive(request(FIRING)).await.unwrap();
    memory.set(0, |t| {
        t.task.as_mut().unwrap().labels.push(NO_ACTION.into());
        t.held_by = Some("restart svc-music".into());
    });
    let resolved = service.receive(request(&resolved())).await.unwrap();
    assert_eq!(resolved.handled(), Handled::Resolved);
    assert_eq!(memory.task(0).task.unwrap().column, "To do");
}

#[tokio::test]
async fn a_close_the_board_refuses_still_resolves_and_records_the_incident() {
    let (config, memory) = (config(), Memory::default());
    let service = Service::new(&config, &memory);
    service.receive(request(FIRING)).await.unwrap();
    memory.set(0, |t| {
        t.task.as_mut().unwrap().labels.push(NO_ACTION.into())
    });
    let member = memory.task(0).task.unwrap().assignee.unwrap();
    *memory.refuses.lock() = Some(member);
    let resolved = service.receive(request(&resolved())).await.unwrap();
    assert_eq!(resolved.handled(), Handled::Resolved);
    assert_eq!(memory.task(0).task.unwrap().column, "To do");
    assert_eq!(memory.incidents.lock().len(), 1, "the incident is recorded");
    assert!(
        !memory.comments.lock().is_empty(),
        "the resolution is commented"
    );
}

#[tokio::test]
async fn a_resolved_group_without_a_task_changes_nothing() {
    let (config, memory) = (config(), Memory::default());
    let answer = Service::new(&config, &memory)
        .receive(request(&resolved()))
        .await
        .unwrap();
    assert_eq!(answer.handled(), Handled::UnknownGroup);
    assert!(memory.tasks.lock().is_empty() && memory.incidents.lock().is_empty());
}

#[tokio::test]
async fn a_request_without_the_token_from_an_unknown_source_or_with_a_bad_body_is_refused() {
    let (config, memory) = (config(), Memory::default());
    let service = Service::new(&config, &memory);
    let mut forged = request(FIRING);
    forged.token = "guess".into();
    assert_eq!(
        service.receive(forged).await.unwrap_err().kind,
        ModuleErrorKind::Denied
    );
    let mut unknown = request(FIRING);
    unknown.source = "github".into();
    assert_eq!(
        service.receive(unknown).await.unwrap_err().kind,
        ModuleErrorKind::NotFound
    );
    let garbage = service.receive(request("{not json")).await.unwrap_err();
    assert_eq!(garbage.kind, ModuleErrorKind::InvalidArgument);
    let pending = service
        .receive(request(&FIRING.replace(
            r#""status": "firing",
  "alerts""#,
            r#""status": "pending",
  "alerts""#,
        )))
        .await
        .unwrap_err();
    assert_eq!(pending.kind, ModuleErrorKind::InvalidArgument);
    assert!(
        memory.tasks.lock().is_empty(),
        "nothing refused reached the board"
    );
}

#[tokio::test]
async fn a_source_that_names_a_workflow_runs_it_and_adds_no_task() {
    let mut config = config();
    config.sources[0].kind = SourceKind::Json.into();
    config.sources[0].board.clear();
    config.sources[0].workflow = "triage".into();
    let memory = Memory::default();
    let service = Service::new(&config, &memory);

    let answer = service
        .receive(request(r#"{"host": "db-1"}"#))
        .await
        .unwrap();

    assert_eq!(answer.handled(), Handled::Run);
    assert_eq!(answer.run, "r-1");
    let runs = memory.runs.lock().clone();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].0, "triage");
    assert_eq!(runs[0].1, "alertmanager");
    assert_eq!(runs[0].2, br#"{"host": "db-1"}"#.to_vec());
    assert!(memory.tasks.lock().is_empty(), "no task");

    let mut wrong = request("{}");
    wrong.token = "nope".into();
    assert!(
        service.receive(wrong).await.is_err(),
        "the token still holds"
    );
}

#[test]
fn a_configuration_the_rules_refuse_names_its_field() {
    let mut bad = config();
    bad.sources[0].board.clear();
    let refused = checked(bad).unwrap_err();
    assert_eq!(refused.kind, ModuleErrorKind::InvalidArgument);
    assert_eq!(refused.field.as_deref(), Some("sources.0.board"));
}

#[tokio::test]
async fn a_heartbeat_is_acknowledged_and_adds_no_task() {
    let (config, memory) = (config(), Memory::default());
    let service = Service::new(&config, &memory);
    let beat = FIRING.replace("PodCrashLooping", "Watchdog");
    let answer = service.receive(request(&beat)).await.unwrap();
    assert_eq!(answer.handled(), Handled::Heartbeat);
    assert!(memory.tasks.lock().is_empty() && memory.told.lock().is_empty());
    let mut unsent = request(&beat);
    unsent.token = "wrong".into();
    let refused = service.receive(unsent).await.unwrap_err();
    assert_eq!(
        refused.kind,
        ModuleErrorKind::Denied,
        "it proves the token too"
    );
}

#[tokio::test]
async fn a_notification_with_no_alerts_does_nothing() {
    let (config, memory) = (config(), Memory::default());
    let service = Service::new(&config, &memory);
    let empty = r#"{"version": "4", "groupKey": "probe", "status": "firing", "receiver": "pcx",
        "groupLabels": {}, "commonLabels": {}, "commonAnnotations": {}, "externalURL": "",
        "alerts": []}"#;

    let answer = service.receive(request(empty)).await.unwrap();

    assert_eq!(answer.handled(), Handled::NoAlerts);
    assert!(answer.task.is_empty());
    assert!(!answer.told);
    assert!(memory.tasks.lock().is_empty(), "no task");
    assert!(memory.told.lock().is_empty(), "no channel");
    assert!(memory.comments.lock().is_empty());
    assert!(memory.incidents.lock().is_empty());
}
