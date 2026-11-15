// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A member's work over ports held in memory.

use std::time::{Duration, Instant};

use async_trait::async_trait;
use parking_lot::Mutex;
use pcx_documents::InMemoryDocuments;
use pcx_module_api::PersonRole;
use pcx_module_api::agent::{TurnEnd, TurnEvent, TurnStop};
use pcx_module_api::model::{StopReason, Usage};
use tokio_util::sync::CancellationToken;

use super::*;
use crate::domain::Authority;
use crate::ports::{Heading, Workplace};
use crate::wake::{Change, Thread, Why};

#[derive(Default)]
struct FakeBoard {
    answered: Mutex<Vec<(String, String, Option<String>)>>,
}

#[async_trait]
impl Board for FakeBoard {
    async fn change(&self, _: &str, id: &str, _: u64) -> Result<Option<Change>, ModuleError> {
        Ok(Some(Change::Comment {
            task: "t1".into(),
            comment: id.into(),
            author: Some("julius".into()),
            text: "@dev can you look?".into(),
            answers: None,
        }))
    }

    async fn heading(&self, _: &str) -> Result<Option<Heading>, ModuleError> {
        Ok(None)
    }

    async fn thread(&self, _: &str) -> Result<Thread, ModuleError> {
        Ok(Thread {
            title: "Ship".into(),
            column: "todo".into(),
            text: "the release".into(),
            comments: vec![("julius".into(), "@dev can you look?".into())],
            decisions: Vec::new(),
        })
    }

    fn answer_tool(&self) -> &str {
        "comment"
    }

    async fn answer(
        &self,
        task: &str,
        text: &str,
        answers: Option<&str>,
    ) -> Result<(), ModuleError> {
        self.answered
            .lock()
            .push((task.into(), text.into(), answers.map(str::to_string)));
        Ok(())
    }
}

struct Skills;

#[async_trait]
impl Library for Skills {
    async fn skill(&self, name: &str) -> Result<Option<String>, ModuleError> {
        Ok((name == "review").then(|| "Read the diff first.".to_string()))
    }
}

#[derive(Default)]
struct Made {
    count: Mutex<u32>,
}

#[async_trait]
impl Sessions for Made {
    async fn create(&self, _: &str) -> Result<String, ModuleError> {
        let mut count = self.count.lock();
        *count += 1;
        Ok(format!("session-{count}"))
    }
}

/// Answers each turn with a fixed text after many events, as a long turn
/// streams (a caller that dropped the receiver does not stop it); keeps
/// the requests.
#[derive(Default)]
struct Agent1 {
    asked: Mutex<Vec<TurnRequest>>,
    /// Each turn's scope: its author and its person's role.
    scoped: Mutex<Vec<(Option<String>, Option<PersonRole>)>>,
}

#[async_trait]
impl Turns for Agent1 {
    async fn run(
        &self,
        scope: &Scope,
        request: &TurnRequest,
        events: &mpsc::Sender<TurnEvent>,
    ) -> Result<TurnEnd, ModuleError> {
        self.asked.lock().push(request.clone());
        self.scoped.lock().push((scope.author.clone(), scope.role));
        for _ in 0..1000 {
            if events.send(TurnEvent::Thinking("hm".into())).await.is_err() {
                break;
            }
        }
        Ok(TurnEnd {
            stop: TurnStop::Answered(StopReason::EndTurn),
            steps: 1,
            usage: Usage::default(),
            answer: " Looks good to me. ".into(),
        })
    }
}

/// Calls tools, the last failing, until it takes its most steps.
struct AtItsMostSteps;

#[async_trait]
impl Turns for AtItsMostSteps {
    async fn run(
        &self,
        _: &Scope,
        _: &TurnRequest,
        events: &mpsc::Sender<TurnEvent>,
    ) -> Result<TurnEnd, ModuleError> {
        for (id, name, failed) in [("a", "read", false), ("b", "write", true)] {
            let call = TurnEvent::ToolCall {
                id: id.into(),
                name: name.into(),
                input: serde_json::json!({"path": "/tmp/probe"}),
            };
            let result = TurnEvent::ToolResult {
                id: id.into(),
                content: if failed { "Permission denied" } else { "ok" }.into(),
                is_error: failed,
            };
            events.send(call).await.unwrap();
            events.send(result).await.unwrap();
        }
        Ok(TurnEnd {
            stop: TurnStop::StepLimit,
            steps: 50,
            usage: Usage::default(),
            answer: String::new(),
        })
    }
}

fn scope() -> Scope {
    Scope {
        tenant: String::new(),
        profile: String::new(),
        session_id: None,
        workspace_id: Some("ws-1".into()),
        author: Some("dev".into()),
        caller_module: None,
        call_depth: 0,
        turn: None,
        reach: None,
        role: None,
        read_only: false,
        person: None,
        deadline: Instant::now() + Duration::from_secs(5),
        cancel: CancellationToken::new(),
    }
}

async fn team(docs: &InMemoryDocuments) {
    let teams = Service::new(docs);
    teams
        .add_agent(Agent {
            name: "dev".into(),
            role: "reviews code".into(),
            authority: Authority::Decide.into(),
            harness: true,
            skills: vec!["review".into(), "missing".into()],
            model: Some("big".into()),
            modules: vec!["tools-git".into()],
            tools: Vec::new(),
            reach: Vec::new(),
        })
        .await
        .unwrap();
    let core = vec!["dev".into()];
    teams.create_team("core", core, "ws-1").await.unwrap();
}

/// A board that names no repository.
struct NoRepository;

#[async_trait]
impl Workplace for NoRepository {
    async fn repository(&self, _task: &str) -> Result<Option<String>, ModuleError> {
        Ok(None)
    }
    async fn worktree(&self, _: &str, _: &str, _: &str) -> Result<String, ModuleError> {
        unreachable!("a board without a repository has no worktree")
    }
    async fn link(&self, _: &str, _: &str) -> Result<(), ModuleError> {
        unreachable!("a board without a repository links no branch")
    }
}

/// A board whose repository is `/work/repo`: the worktrees made and the
/// branches linked.
#[derive(Default)]
struct Repository {
    made: Mutex<Vec<(String, String)>>,
    linked: Mutex<Vec<(String, String)>>,
}

#[async_trait]
impl Workplace for Repository {
    async fn repository(&self, _task: &str) -> Result<Option<String>, ModuleError> {
        Ok(Some("/work/repo".into()))
    }
    async fn worktree(&self, repo: &str, path: &str, branch: &str) -> Result<String, ModuleError> {
        self.made
            .lock()
            .push((path.to_string(), branch.to_string()));
        Ok(format!("{repo}/{path}"))
    }
    async fn link(&self, task: &str, branch: &str) -> Result<(), ModuleError> {
        self.linked
            .lock()
            .push((task.to_string(), branch.to_string()));
        Ok(())
    }
}

#[tokio::test]
async fn a_member_works_in_a_worktree_of_its_own_made_once() {
    let docs = InMemoryDocuments::new(None, post_cortex_proto::TEAM_DESCRIPTOR_SET).unwrap();
    team(&docs).await;
    let (board, sessions, agent, scope, repository) = (
        FakeBoard::default(),
        Made::default(),
        Agent1::default(),
        scope(),
        Repository::default(),
    );
    let worker = Worker {
        docs: &docs,
        board: &board,
        library: &Skills,
        sessions: &sessions,
        turns: &agent,
        workplace: &repository,
        scope: &scope,
    };
    let woken = worker.woken("board/comment", "c1", 1).await.unwrap();
    worker
        .work(&woken[0], &crate::progress::Progress::default())
        .await
        .unwrap();
    worker
        .work(&woken[0], &crate::progress::Progress::default())
        .await
        .unwrap();
    assert_eq!(
        repository.made.lock().clone(),
        [(".worktrees/t1".to_string(), "task/t1-ship".to_string())],
        "made once, kept for the next turn"
    );
    assert_eq!(
        repository.linked.lock().clone(),
        [("t1".to_string(), "task/t1-ship".to_string())]
    );
    let asked = agent.asked.lock().clone();
    for turn in &asked {
        assert!(
            turn.prompt.starts_with("Work in your worktree of the task's repository, /work/repo/.worktrees/t1, on its branch task/t1-ship"),
            "{}",
            turn.prompt
        );
    }
    let kept: Vec<Item<Work>> = Typed::new(&docs, Kind::Work.document())
        .all(None)
        .await
        .unwrap();
    assert_eq!(kept[0].data.worktree, "/work/repo/.worktrees/t1");
    assert_eq!(kept[0].data.branch, "task/t1-ship");
}

#[tokio::test]
async fn a_mentioned_member_works_in_its_session_and_answers_in_the_thread() {
    let docs = InMemoryDocuments::new(None, post_cortex_proto::TEAM_DESCRIPTOR_SET).unwrap();
    team(&docs).await;
    let (board, sessions, agent, scope) = (
        FakeBoard::default(),
        Made::default(),
        Agent1::default(),
        scope(),
    );
    let worker = Worker {
        docs: &docs,
        board: &board,
        library: &Skills,
        sessions: &sessions,
        turns: &agent,
        workplace: &NoRepository,
        scope: &scope,
    };
    let woken = worker.woken("board/comment", "c1", 1).await.unwrap();
    assert_eq!(woken.len(), 1);
    assert_eq!(
        (woken[0].member.as_str(), woken[0].why),
        ("dev", Why::Mentioned)
    );

    // A turn of many events ends: no reader holds it back.
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        worker.work(&woken[0], &crate::progress::Progress::default()),
    )
    .await
    .expect("the member's turn waited for a reader")
    .unwrap();
    worker
        .work(&woken[0], &crate::progress::Progress::default())
        .await
        .unwrap();
    let asked = agent.asked.lock().clone();
    assert_eq!(asked[0].session, "session-1");
    assert_eq!(
        asked[1].session, "session-1",
        "the member's session for the task is kept"
    );
    let kept: Vec<Item<Work>> = Typed::new(&docs, Kind::Work.document())
        .all(None)
        .await
        .unwrap();
    assert_eq!(kept[0].data.turns, 2, "each turn is counted");
    assert_eq!(asked[0].model.as_deref(), Some("big"));
    assert_eq!(asked[0].modules, ["tools-git"]);
    assert_eq!(
        asked[0].withheld,
        ["comment"],
        "the member's answer is its comment: its turn does not comment"
    );
    assert!(
        asked[0]
            .instructions
            .ends_with("## Skill: review\nRead the diff first.")
    );
    assert!(asked[0].prompt.contains("# Ship [todo]"));
    assert!(
        asked[0].prompt.contains("it is ws-1"),
        "the turn is told its workspace"
    );
    assert_eq!(
        board.answered.lock()[0],
        (
            "t1".to_string(),
            "Looks good to me.".to_string(),
            Some("c1".to_string())
        )
    );
}

#[tokio::test]
async fn a_turn_at_its_most_steps_says_so_with_its_last_steps_and_session() {
    let docs = InMemoryDocuments::new(None, post_cortex_proto::TEAM_DESCRIPTOR_SET).unwrap();
    team(&docs).await;
    let (board, sessions, scope) = (FakeBoard::default(), Made::default(), scope());
    let worker = Worker {
        docs: &docs,
        board: &board,
        library: &Skills,
        sessions: &sessions,
        turns: &AtItsMostSteps,
        workplace: &NoRepository,
        scope: &scope,
    };
    let woken = worker.woken("board/comment", "c1", 1).await.unwrap();
    let end = worker
        .work(&woken[0], &crate::progress::Progress::default())
        .await
        .unwrap();
    assert_eq!(end.stop, TurnStop::StepLimit);
    let answered = board.answered.lock().clone();
    assert_eq!(answered.len(), 1, "the member says how its turn ended");
    let (task, said, answers) = &answered[0];
    assert_eq!((task.as_str(), answers.as_deref()), ("t1", Some("c1")));
    assert!(said.starts_with("dev stopped after 50 steps"), "{said}");
    assert!(
        said.contains(r#"- `→ read {"path":"/tmp/probe"}`"#),
        "{said}"
    );
    assert!(
        said.contains(r#"- `→ write {"path":"/tmp/probe"} ✗ Permission denied`"#),
        "{said}"
    );
    assert!(said.contains("session `session-1`"), "{said}");
}

mod reach;
#[tokio::test]
async fn a_person_talks_with_a_member_in_its_role_and_as_it() {
    let docs = InMemoryDocuments::new(None, post_cortex_proto::TEAM_DESCRIPTOR_SET).unwrap();
    team(&docs).await;
    Service::new(&docs)
        .add_agent(Agent {
            name: "claude-code".into(),
            role: "engineer".into(),
            authority: Authority::Discuss.into(),
            harness: false,
            skills: Vec::new(),
            model: None,
            modules: Vec::new(),
            tools: Vec::new(),
            reach: Vec::new(),
        })
        .await
        .unwrap();
    Service::new(&docs)
        .create_team("outside", vec!["claude-code".into()], "ws")
        .await
        .unwrap();
    let (board, sessions, agent, scope) = (
        FakeBoard::default(),
        Made::default(),
        Agent1::default(),
        Scope {
            author: Some("ana".into()),
            role: Some(PersonRole::Owner),
            ..scope()
        },
    );
    let worker = Worker {
        docs: &docs,
        board: &board,
        library: &Skills,
        sessions: &sessions,
        turns: &agent,
        workplace: &NoRepository,
        scope: &scope,
    };
    let (events, told) = mpsc::channel(8);
    drop(told);
    let end = worker
        .talk("dev", "chat-1", "what is left?", &events)
        .await
        .unwrap();
    assert_eq!(end.steps, 1);
    let asked = agent.asked.lock()[0].clone();
    assert_eq!(
        (asked.session.as_str(), asked.prompt.as_str()),
        ("chat-1", "what is left?")
    );
    assert_eq!(
        asked.speaker.as_deref(),
        Some("dev"),
        "written as the member"
    );
    assert_eq!(asked.model.as_deref(), Some("big"));
    assert!(asked.withheld.is_empty(), "a talk withholds nothing");
    assert!(asked.instructions.contains("## Skill: review"));
    assert!(
        asked.instructions.ends_with(
            "You work in workspace ws-1: name it (workspace_id) in every search of memory."
        ),
        "told its workspace: {}",
        asked.instructions
    );
    assert_eq!(
        asked.asker.as_deref(),
        Some("ana"),
        "the person writes the prompt"
    );
    assert_eq!(
        agent.scoped.lock()[0],
        (Some("dev".to_string()), None),
        "its calls are the member's, under no person's role"
    );
    assert!(board.answered.lock().is_empty(), "a talk posts no comment");

    let external = worker
        .talk("claude-code", "chat-2", "hi", &events)
        .await
        .unwrap_err();
    assert_eq!(external.kind, ModuleErrorKind::FailedPrecondition);
    let nobody = worker
        .talk("nobody", "chat-3", "hi", &events)
        .await
        .unwrap_err();
    assert_eq!(nobody.kind, ModuleErrorKind::NotFound);
}
