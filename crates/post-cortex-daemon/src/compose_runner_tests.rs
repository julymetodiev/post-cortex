// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A task whose runner is offline waits (docs/connect.md section 7): a
//! member's work on a task of a board with a repository needs `tools-git`,
//! which runs on a runner (`pcx_host::RemoteModule`); offline, the task is
//! labeled waiting, no try spent and not blocked; once the runner connects
//! (`EVENT_KIND_MODULE_AVAILABLE`), the label goes and the work runs. And
//! a task's thread, asked by a person who names the workspace only in the
//! request, reads its branch's commits from the runner, which takes only a
//! call that names the workspace it serves.

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use pcx_agent::AgentModule;
use pcx_board::BoardModule;
use pcx_host::{Host, RemoteModule};
use pcx_module_api::model::Block;
use pcx_module_api::{EventKind, Module, ModuleAvailable, ModuleError, ModuleErrorKind, Scope};
use pcx_skills::SkillsModule;
use pcx_team::TeamModule;
use pcx_tools_git::GitModule;
use post_cortex_memory::capabilities::CreateWorkspace;
use post_cortex_proto::tools_git::git_server::{Git, GitServer};
use post_cortex_proto::tools_git::{
    AddWorktreeRequest, BranchCommitsRequest, BranchList, BranchesRequest, Commit, CommitRequest,
    CommitResponse, Commits, CreateBranchRequest, CreateBranchResponse, DiffRequest, DiffResponse,
    LogRequest, PushRequest, Pushed, StatusRequest, StatusResponse, SwitchRequest, SwitchResponse,
    Worktree,
};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use tonic::{Request, Response, Status};
use tower::ServiceExt;

use crate::compose_scripted_tests::{Provider, Scripted};

/// The runner's git: a worktree is made, a branch has one commit
/// ([`ON_THE_BRANCH`]); anything else answers empty.
struct RunnerGit;

/// The one commit of every branch of [`RunnerGit`].
const ON_THE_BRANCH: &str = "c0ffee";

fn unasked<T: Default>() -> Result<Response<T>, Status> {
    Ok(Response::new(T::default()))
}

#[tonic::async_trait]
impl Git for RunnerGit {
    async fn status(&self, _: Request<StatusRequest>) -> Result<Response<StatusResponse>, Status> {
        unasked()
    }
    async fn diff(&self, _: Request<DiffRequest>) -> Result<Response<DiffResponse>, Status> {
        unasked()
    }
    async fn log(&self, _: Request<LogRequest>) -> Result<Response<Commits>, Status> {
        unasked()
    }
    async fn branches(&self, _: Request<BranchesRequest>) -> Result<Response<BranchList>, Status> {
        unasked()
    }
    async fn create_branch(
        &self,
        _: Request<CreateBranchRequest>,
    ) -> Result<Response<CreateBranchResponse>, Status> {
        unasked()
    }
    async fn switch(&self, _: Request<SwitchRequest>) -> Result<Response<SwitchResponse>, Status> {
        unasked()
    }
    async fn commit(&self, _: Request<CommitRequest>) -> Result<Response<CommitResponse>, Status> {
        unasked()
    }
    async fn branch_commits(
        &self,
        _: Request<BranchCommitsRequest>,
    ) -> Result<Response<Commits>, Status> {
        Ok(Response::new(Commits {
            commits: vec![Commit {
                sha: ON_THE_BRANCH.to_string(),
                ..Commit::default()
            }],
        }))
    }
    async fn add_worktree(
        &self,
        request: Request<AddWorktreeRequest>,
    ) -> Result<Response<Worktree>, Status> {
        let asked = request.into_inner();
        Ok(Response::new(Worktree {
            path: format!("/repo/{}", asked.path),
            branch: asked.branch,
            existing: false,
        }))
    }
    async fn push(&self, _: Request<PushRequest>) -> Result<Response<Pushed>, Status> {
        unasked()
    }
}

fn scope(author: &str) -> Scope {
    Scope {
        tenant: String::new(),
        profile: String::new(),
        session_id: None,
        workspace_id: None,
        author: Some(author.to_string()),
        caller_module: None,
        call_depth: 0,
        turn: None,
        reach: None,
        role: None,
        read_only: false,
        person: None,
        deadline: Instant::now() + Duration::from_secs(20),
        cancel: CancellationToken::new(),
    }
}

async fn call(host: &Host, tool: &str, input: Value) -> Value {
    crate::memory_tests::as_person(host, &scope("julius"), tool, input)
        .await
        .unwrap()
}

/// The task's thread, once `done` says it is as awaited.
async fn thread(host: &Host, ws: &str, task: &Value, done: impl Fn(&Value) -> bool) -> Value {
    let mut thread = Value::Null;
    for _ in 0..200 {
        thread = call(
            host,
            "task_get",
            json!({ "workspace_id": ws, "task": task["id"] }),
        )
        .await;
        if done(&thread) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    thread
}

#[tokio::test]
async fn a_task_whose_runner_is_offline_waits_and_goes_on_when_it_connects() {
    let data = tempfile::tempdir().unwrap();
    let memory = Arc::new(
        crate::memory_tests::rocksdb_memory(
            &data,
            post_cortex_memory::SystemConfig {
                enable_embeddings: false,
                ..Default::default()
            },
        )
        .await
        .unwrap(),
    );
    let ws = memory
        .workspaces
        .create(CreateWorkspace {
            name: "team".into(),
            description: String::new(),
        })
        .await
        .unwrap()
        .id
        .to_string();
    // The runner's tools-git, offline until `online`.
    let online = Arc::new(AtomicBool::new(false));
    let git = GitServer::new(RunnerGit);
    let to_runner = {
        let online = Arc::clone(&online);
        pcx_module_api::grpc::channel(tower::service_fn(
            move |request: axum::http::Request<tonic::body::Body>| {
                let (online, git) = (online.load(Ordering::SeqCst), git.clone());
                async move {
                    if !online {
                        let offline =
                            ModuleError::new(ModuleErrorKind::Offline, "the runner is offline");
                        return Ok::<_, Infallible>(
                            Status::from(offline).into_http::<tonic::body::Body>(),
                        );
                    }
                    git.oneshot(request).await
                }
            },
        ))
    };
    let scripted = Scripted::with(vec![vec![Block::Text {
        text: "Done.".into(),
    }]]);
    let modules: Vec<Arc<dyn Module>> = vec![
        crate::memory_tests::memory_module(&memory).0,
        Arc::new(Provider(Arc::clone(&scripted))),
        Arc::new(BoardModule::new()),
        Arc::new(SkillsModule::new()),
        Arc::new(AgentModule::new()),
        Arc::new(TeamModule::new()),
        Arc::new(RemoteModule::new(
            GitModule::new().manifest().clone(),
            to_runner,
        )),
    ];
    let config = HashMap::from([
        (SkillsModule::ID.to_string(), json!({ "workspace_id": ws })),
        (TeamModule::ID.to_string(), json!({ "tries": 2 })),
    ]);
    let host = Arc::new(
        Host::start(modules, &config, &HashMap::new())
            .await
            .unwrap(),
    );
    let stop = CancellationToken::new();
    let feed = tokio::spawn(crate::compose::feed_memory_writes(
        Arc::clone(&host),
        Arc::clone(&memory),
        stop.clone(),
    ));
    call(&host, "agent_add", json!({ "workspace_id": ws, "agent": { "name": "dev", "role": "writes code", "authority": "AUTHORITY_DISCUSS", "harness": true } })).await;
    call(
        &host,
        "team_create",
        json!({ "workspace_id": ws, "name": "core", "members": ["dev"] }),
    )
    .await;
    let board = call(
        &host,
        "board_create",
        json!({ "workspace_id": ws, "name": "Sprint", "columns": ["todo", "done"], "repo": "/repo" }),
    )
    .await;
    let task = call(
        &host,
        "task_add",
        json!({ "workspace_id": ws, "board": board["id"], "title": "Ship", "assignee": "dev" }),
    )
    .await;

    // Offline: waiting, not blocked, no turn taken.
    // The team labels the task, then says why on it: two writes, so both
    // are waited for.
    let waiting = thread(&host, &ws, &task, |t| {
        t["task"]["task"]["labels"]
            .as_array()
            .is_some_and(|l| l.iter().any(|x| x == "waiting-runner"))
            && t["comments"][0]["text"].is_string()
    })
    .await;
    assert_eq!(
        waiting["task"]["task"]["labels"],
        json!(["waiting-runner"]),
        "{waiting}"
    );
    let said = waiting["comments"][0]["text"].as_str().unwrap();
    assert!(
        said.starts_with("Waiting: dev's work needs the runner"),
        "{said}"
    );
    assert!(scripted.asked.lock().is_empty(), "no turn while it waits");

    // The runner connects: the label goes and the work runs.
    online.store(true, Ordering::SeqCst);
    host.dispatch(
        &scope("pcx"),
        EventKind::ModuleAvailable,
        &ModuleAvailable {
            modules: vec![GitModule::ID.to_string()],
        }
        .to_event(),
    )
    .await;
    let resumed = thread(&host, &ws, &task, |t| {
        t["task"]["task"]["labels"]
            .as_array()
            .is_some_and(|l| l.is_empty())
    })
    .await;
    assert_eq!(resumed["task"]["task"]["labels"], json!([]), "{resumed}");
    for _ in 0..200 {
        if !scripted.asked.lock().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let after = call(
        &host,
        "task_get",
        json!({ "workspace_id": ws, "task": task["id"] }),
    )
    .await;
    assert_eq!(
        scripted.asked.lock().len(),
        1,
        "the work ran once it could: {after}"
    );

    stop.cancel();
    feed.await.unwrap();
}

#[tokio::test]
async fn a_tasks_thread_asked_in_a_workspace_the_request_names_reads_its_commits_from_the_runner() {
    let data = tempfile::tempdir().unwrap();
    let memory = Arc::new(
        crate::memory_tests::rocksdb_memory(
            &data,
            post_cortex_memory::SystemConfig {
                enable_embeddings: false,
                ..Default::default()
            },
        )
        .await
        .unwrap(),
    );
    let ws = memory
        .workspaces
        .create(CreateWorkspace {
            name: "team".into(),
            description: String::new(),
        })
        .await
        .unwrap()
        .id
        .to_string();
    // The runner's tools-git, which takes only a call that names the
    // workspace it serves, as the cloud's gateway does.
    let git = GitServer::new(RunnerGit);
    let to_runner = {
        let serves = ws.clone();
        pcx_module_api::grpc::channel(tower::service_fn(
            move |request: axum::http::Request<tonic::body::Body>| {
                let named = request
                    .headers()
                    .get(pcx_module_api::metadata::WORKSPACE)
                    .and_then(|v| v.to_str().ok())
                    == Some(serves.as_str());
                let git = git.clone();
                async move {
                    if !named {
                        let refused = Status::permission_denied("not the runner's workspace");
                        return Ok::<_, Infallible>(refused.into_http::<tonic::body::Body>());
                    }
                    git.oneshot(request).await
                }
            },
        ))
    };
    let modules: Vec<Arc<dyn Module>> = vec![
        crate::memory_tests::memory_module(&memory).0,
        Arc::new(BoardModule::new()),
        Arc::new(RemoteModule::new(
            GitModule::new().manifest().clone(),
            to_runner,
        )),
    ];
    let host = Host::start(modules, &HashMap::new(), &HashMap::new())
        .await
        .unwrap();
    // A person's calls: the workspace is in the request, not the scope.
    let board = call(
        &host,
        "board_create",
        json!({ "workspace_id": ws, "name": "Sprint", "columns": ["todo", "done"], "repo": "/repo" }),
    )
    .await;
    let task = call(
        &host,
        "task_add",
        json!({ "workspace_id": ws, "board": board["id"], "title": "Ship" }),
    )
    .await;
    call(
        &host,
        "task_update",
        json!({ "workspace_id": ws, "task": task["id"], "version": task["version"], "branch": "task/ship" }),
    )
    .await;

    let thread = call(
        &host,
        "task_get",
        json!({ "workspace_id": ws, "task": task["id"] }),
    )
    .await;
    assert_eq!(
        thread["task"]["task"]["links"]["commits"],
        json!([ON_THE_BRANCH]),
        "{thread}"
    );
}
