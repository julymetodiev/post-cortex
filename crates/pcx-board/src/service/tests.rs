// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The board service over ports held in memory.

use async_trait::async_trait;
use parking_lot::Mutex;

use super::*;

use crate::domain::{DecisionRules, Risk};
use crate::ports::DecisionRecord;
use pcx_documents::{InMemoryDocuments, Typed};
use pcx_team::Stakes;
use post_cortex_proto::board::{AddTaskRequest, MoveTaskRequest, UpdateTaskRequest};

/// Sessions, decision records and events, kept.
#[derive(Default)]
struct Memory {
    sessions: Mutex<Vec<String>>,
    decisions: Mutex<Vec<(String, DecisionRecord)>>,
    events: Mutex<Vec<BoardEvent>>,
}

#[async_trait]
impl Events for Memory {
    async fn emitted(&self, event: BoardEvent) {
        self.events.lock().push(event);
    }
}

#[async_trait]
impl Records for Memory {
    async fn create_session(&self, name: &str) -> Result<String, ModuleError> {
        let mut sessions = self.sessions.lock();
        sessions.push(name.to_string());
        Ok(format!("session-{}", sessions.len()))
    }

    async fn record_decision(
        &self,
        session: &str,
        decision: &DecisionRecord,
    ) -> Result<(), ModuleError> {
        self.decisions
            .lock()
            .push((session.to_string(), decision.clone()));
        Ok(())
    }
}

/// Every branch has the same two commits.
struct Git;

#[async_trait]
impl Commits for Git {
    async fn branch_commits(&self, _repo: &str, branch: &str) -> Result<Vec<String>, ModuleError> {
        Ok(vec![format!("{branch}-2"), format!("{branch}-1")])
    }
}

/// A move of `task` from `version` to the end of `column`.
fn to(task: &str, version: u64, column: &str) -> MoveTaskRequest {
    MoveTaskRequest {
        task: task.to_string(),
        version,
        column: column.to_string(),
        ..MoveTaskRequest::default()
    }
}

fn columns() -> Vec<String> {
    vec!["todo".into(), "doing".into(), "done".into()]
}

/// Empty documents with the board's kinds (and the team's messages).
fn documents() -> InMemoryDocuments {
    InMemoryDocuments::new(None, post_cortex_proto::BOARD_DESCRIPTOR_SET).unwrap()
}

#[tokio::test]
async fn tasks_move_across_the_columns_and_a_stale_move_conflicts() {
    let (docs, memory) = (documents(), Memory::default());
    let board = Service::new(&docs, &memory, &Git, &memory, Some("ana"), None);
    let created = board
        .create_board("Sprint", columns(), vec![], Some("/work/repo".into()))
        .await
        .unwrap();
    assert_eq!(created.data.session_id, "session-1");
    assert_eq!(memory.sessions.lock()[0], "Board: Sprint");

    let task = board
        .add_task(AddTaskRequest {
            board: created.document.id.clone(),
            title: "Ship".into(),
            text: "the release".into(),
            ..Default::default()
        })
        .await
        .unwrap()
        .task
        .task;
    assert_eq!(task.data.column, "todo");

    let moved = board
        .move_task(to(&task.document.id, task.document.version, "doing"))
        .await
        .unwrap()
        .task;
    assert_eq!(
        (moved.data.column.as_str(), moved.document.version),
        ("doing", 2)
    );
    let stale = board
        .move_task(to(&task.document.id, task.document.version, "done"))
        .await
        .unwrap_err();
    assert_eq!(stale.kind, ModuleErrorKind::Conflict);
    let nowhere = board
        .move_task(to(&task.document.id, moved.document.version, "later"))
        .await
        .unwrap_err();
    assert_eq!(nowhere.kind, ModuleErrorKind::InvalidArgument);

    let doing = board
        .tasks(&created.document.id, Some("doing"))
        .await
        .unwrap()
        .tasks;
    assert_eq!(doing.len(), 1);
    assert!(
        board
            .tasks(&created.document.id, Some("todo"))
            .await
            .unwrap()
            .tasks
            .is_empty()
    );

    let linked = board
        .update_task(UpdateTaskRequest {
            task: task.document.id.clone(),
            version: moved.document.version,
            branch: Some("task-1".into()),
            ..Default::default()
        })
        .await
        .unwrap()
        .task;
    assert_eq!(
        linked.data.links.as_ref().unwrap().commits,
        ["task-1-2", "task-1-1"]
    );
    assert_eq!(linked.document.text, "the release");
}

/// A branch whose commits change, or a runner that is offline (`None`).
struct Branch(Mutex<Option<Vec<String>>>);

#[async_trait]
impl Commits for Branch {
    async fn branch_commits(&self, _repo: &str, _branch: &str) -> Result<Vec<String>, ModuleError> {
        self.0
            .lock()
            .clone()
            .ok_or_else(|| ModuleError::new(ModuleErrorKind::Unavailable, "runner offline"))
    }
}

#[tokio::test]
async fn a_tasks_thread_shows_its_branchs_commits_as_they_are_now() {
    let (docs, memory) = (documents(), Memory::default());
    let git = Branch(Mutex::new(Some(Vec::new())));
    let board = Service::new(&docs, &memory, &git, &memory, Some("ana"), None);
    let created = board
        .create_board("Sprint", columns(), vec![], Some("/work/repo".into()))
        .await
        .unwrap();
    let task = board
        .add_task(AddTaskRequest {
            board: created.document.id.clone(),
            title: "Ship".into(),
            ..Default::default()
        })
        .await
        .unwrap()
        .task
        .task;
    // Linked as a worktree is made: the branch has no commit of its own yet.
    board
        .update_task(UpdateTaskRequest {
            task: task.document.id.clone(),
            version: task.document.version,
            branch: Some("task/ship".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    let commits = |thread: &Thread| {
        thread
            .task
            .task
            .data
            .links
            .as_ref()
            .unwrap()
            .commits
            .clone()
    };

    *git.0.lock() = Some(vec!["b2".into(), "b1".into()]);
    let now = board.thread(&task.document.id).await.unwrap();
    assert_eq!(
        (commits(&now), now.commits_unread),
        (vec!["b2".to_string(), "b1".to_string()], None)
    );

    *git.0.lock() = None;
    let offline = board.thread(&task.document.id).await.unwrap();
    assert_eq!(
        (commits(&offline), offline.commits_unread.as_deref()),
        (Vec::<String>::new(), Some("runner offline")),
        "the ones kept when it was linked, and why the branch's were not read"
    );
}

#[tokio::test]
async fn a_task_is_named_by_its_short_id_as_the_app_shows_it() {
    let (docs, memory) = (documents(), Memory::default());
    let board = Service::new(&docs, &memory, &Git, &memory, Some("ana"), None);
    let on = board
        .create_board("Sprint", columns(), vec![], None)
        .await
        .unwrap();
    let add = |title: &str, depends_on: Vec<String>| AddTaskRequest {
        board: on.document.id.clone(),
        title: title.into(),
        depends_on,
        ..Default::default()
    };
    let first = board
        .add_task(add("first", vec![]))
        .await
        .unwrap()
        .task
        .task;
    // The memory of these tests names a task `board/task-<n>`: its short id
    // names it while it is the only task.
    let short = first.document.id[..domain::SHORT_ID].to_string();
    let moved = board.move_task(to(&short, 1, "doing")).await.unwrap().task;
    assert_eq!(moved.document.id, first.document.id);
    assert_eq!(moved.data.column, "doing");
    let thread = board.thread(&short).await.unwrap();
    assert_eq!(thread.task.task.document.id, first.document.id);
    let said = board
        .comment(&short, "seen", None, Vec::new())
        .await
        .unwrap();
    assert_eq!(said.data.task, first.document.id);
    // A dependency by its short id is kept by its full one.
    let second = board
        .add_task(add("second", vec![short.clone()]))
        .await
        .unwrap()
        .task
        .task;
    assert_eq!(
        second.data.depends_on,
        std::slice::from_ref(&first.document.id)
    );

    let both = board.move_task(to(&short, 2, "done")).await.unwrap_err();
    assert_eq!(both.kind, ModuleErrorKind::InvalidArgument);
    assert!(both.message.contains(&second.document.id));
    let unknown = board
        .move_task(to("ffffffff", 1, "doing"))
        .await
        .unwrap_err();
    assert_eq!(unknown.kind, ModuleErrorKind::NotFound);
}

#[tokio::test]
async fn a_risky_decision_holds_its_task_until_a_person_approves_it() {
    let (docs, memory) = (documents(), Memory::default());
    let agent = Service::new(&docs, &memory, &Git, &memory, Some("agent"), None);
    let created = agent
        .create_board("Ops", columns(), vec![], None)
        .await
        .unwrap();
    let task = agent
        .add_task(AddTaskRequest {
            board: created.document.id.clone(),
            title: "Clean up".into(),
            ..Default::default()
        })
        .await
        .unwrap()
        .task
        .task;

    let decision = agent
        .decide(
            &task.document.id,
            "drop the old table",
            "nothing reads it",
            Risk::Data,
            Stakes::default(),
        )
        .await
        .unwrap();
    let (session, record) = memory.decisions.lock()[0].clone();
    assert_eq!(session, created.data.session_id);
    assert_eq!(
        (record.task.as_str(), record.board.as_str()),
        ("Clean up", "Ops")
    );

    let held = agent
        .move_task(to(&task.document.id, task.document.version, "done"))
        .await
        .unwrap_err();
    assert_eq!(held.kind, ModuleErrorKind::FailedPrecondition);
    assert!(held.message.contains(&decision.document.id), "{held}");
    // Talking about it goes on.
    agent
        .comment(
            &task.document.id,
            "is anything still reading it?",
            None,
            Vec::new(),
        )
        .await
        .unwrap();
    agent
        .update_task(UpdateTaskRequest {
            task: task.document.id.clone(),
            version: task.document.version,
            text: Some("drop it after the backup".into()),
            ..Default::default()
        })
        .await
        .unwrap();

    let person = Service::new(&docs, &memory, &Git, &memory, Some("julius"), None);
    let approved = person
        .approve(&decision.document.id, decision.document.version, None)
        .await
        .unwrap();
    assert_eq!(approved.data.approved_by.as_deref(), Some("julius"));
    let thread = person.thread(&task.document.id).await.unwrap();
    assert_eq!(thread.comments.len(), 1);
    assert_eq!(
        thread.decisions[0].data.approved_by.as_deref(),
        Some("julius")
    );
    person
        .move_task(to(
            &task.document.id,
            thread.task.task.document.version,
            "done",
        ))
        .await
        .unwrap();
}

#[tokio::test]
async fn decisions_name_their_maker_and_branches_need_a_repository() {
    let (docs, memory) = (documents(), Memory::default());
    let anonymous = Service::new(&docs, &memory, &Git, &memory, None, None);
    let created = anonymous
        .create_board("B", columns(), vec![], None)
        .await
        .unwrap();
    let task = anonymous
        .add_task(AddTaskRequest {
            board: created.document.id.clone(),
            title: "T".into(),
            ..Default::default()
        })
        .await
        .unwrap()
        .task
        .task;
    let unnamed = anonymous
        .decide(&task.document.id, "x", "y", Risk::None, Stakes::default())
        .await
        .unwrap_err();
    assert_eq!(unnamed.kind, ModuleErrorKind::FailedPrecondition);
    let no_repo = anonymous
        .update_task(UpdateTaskRequest {
            task: task.document.id.clone(),
            version: task.document.version,
            branch: Some("b".into()),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(no_repo.kind, ModuleErrorKind::FailedPrecondition);
    assert!(memory.decisions.lock().is_empty());
    let refused = anonymous
        .create_board("B", vec![], vec![], None)
        .await
        .unwrap_err();
    assert_eq!(refused.kind, ModuleErrorKind::InvalidArgument);
    assert_eq!(
        memory.sessions.lock().len(),
        1,
        "no session for a refused board"
    );
}

/// A team of `ana` (close) and `bot` (discuss) in the documents.
async fn team(docs: &InMemoryDocuments) {
    let member = |name: &str, authority: pcx_team::Authority| pcx_team::Agent {
        name: name.into(),
        role: "works".into(),
        authority: authority.into(),
        harness: false,
        skills: Vec::new(),
        model: None,
        modules: Vec::new(),
        tools: Vec::new(),
        reach: Vec::new(),
    };
    let agents = Typed::new(docs, pcx_team::TeamKind::Agent.document());
    agents
        .create(&member("ana", pcx_team::Authority::Close), "")
        .await
        .unwrap();
    agents
        .create(&member("bot", pcx_team::Authority::Discuss), "")
        .await
        .unwrap();
    Typed::new(docs, pcx_team::TeamKind::Team.document())
        .create(
            &pcx_team::Team {
                name: "core".into(),
                members: vec!["ana".into(), "bot".into()],
            },
            "",
        )
        .await
        .unwrap();
}

mod events;
mod plans;
mod team;

#[tokio::test]
async fn a_board_is_renamed_and_its_repository_changed_from_the_version_read() {
    let (docs, memory) = (documents(), Memory::default());
    let ana = Service::new(&docs, &memory, &Git, &memory, Some("ana"), None);
    let created = ana
        .create_board("Sprint", columns(), vec![], Some("/work/repo".into()))
        .await
        .unwrap();
    let id = created.document.id.as_str();
    let moved = ana
        .update_board(
            id,
            created.document.version,
            None,
            Some("/work/other".into()),
        )
        .await
        .unwrap();
    assert_eq!(moved.data.repo.as_deref(), Some("/work/other"));
    assert_eq!(moved.data.name, "Sprint");
    assert_eq!(moved.data.columns, created.data.columns, "its columns stay");
    assert_eq!(moved.data.session_id, created.data.session_id);

    let stale = ana
        .update_board(id, created.document.version, Some("Old".into()), None)
        .await
        .unwrap_err();
    assert_eq!(stale.kind, ModuleErrorKind::Conflict);
    let renamed = ana
        .update_board(
            id,
            moved.document.version,
            Some(" Ops ".into()),
            Some(" ".into()),
        )
        .await
        .unwrap();
    assert_eq!(renamed.data.name, "Ops");
    assert_eq!(renamed.data.repo, None, "an empty repository is none");
    let empty = ana
        .update_board(id, renamed.document.version, Some(String::new()), None)
        .await
        .unwrap_err();
    assert_eq!(empty.kind, ModuleErrorKind::InvalidArgument);
}
