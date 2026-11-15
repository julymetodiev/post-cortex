// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The scheduler as the daemon composes it (ARCHITECTURE section 8,
//! operations, W28): a job kept as a document in memory, run by hand, adds
//! its task to the real board once; its run is kept; its name is unique;
//! a probe with no token reads whether it runs on time.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use pcx_board::BoardModule;
use pcx_host::{Host, Origin};
use pcx_module_api::{Module, ModuleErrorKind, Scope};
use pcx_scheduler::SchedulerModule;
use post_cortex_memory::capabilities::CreateWorkspace;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

fn scope() -> Scope {
    Scope {
        tenant: String::new(),
        profile: String::new(),
        session_id: None,
        workspace_id: None,
        author: Some("julius".to_string()),
        caller_module: None,
        call_depth: 0,
        turn: None,
        reach: None,
        role: None,
        read_only: false,
        person: None,
        deadline: Instant::now() + Duration::from_secs(10),
        cancel: CancellationToken::new(),
    }
}

async fn call(host: &Host, tool: &str, input: Value) -> Value {
    host.call_for_agent(&scope(), tool, input)
        .await
        .unwrap()
        .data
        .unwrap()
}

#[tokio::test]
async fn a_job_run_by_hand_adds_its_task_to_the_board_once() {
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
            name: "Equal".into(),
            description: String::new(),
        })
        .await
        .unwrap()
        .id
        .to_string();
    let modules: Vec<Arc<dyn Module>> = vec![
        crate::memory_tests::memory_module(&memory).0,
        Arc::new(BoardModule::new()),
        Arc::new(SchedulerModule::new()),
    ];
    let config = HashMap::from([(
        SchedulerModule::ID.to_string(),
        json!({ "workspace_id": ws }),
    )]);
    let host = Host::start(modules, &config, &HashMap::new())
        .await
        .unwrap();
    let board = call(
        &host,
        "board_create",
        json!({"workspace_id": ws, "name": "eq-ops", "columns": ["Triage", "Done"]}),
    )
    .await;
    let board_id = board["id"].as_str().unwrap();

    let job = json!({"job": {
        "name": "daily-report",
        "cron": {"expression": "0 9 * * *", "time_zone": "Europe/Sofia"},
        "task": {"workspace_id": ws, "board": board_id, "title": "Daily report",
                 "assignee": "eq-sentinel"}}});
    let added = call(&host, "job_add", job.clone()).await;
    assert!(added["next"].is_string(), "{added}");
    let taken = host
        .call_for_agent(&scope(), "job_add", job)
        .await
        .unwrap_err();
    assert_eq!(taken.kind, ModuleErrorKind::Conflict);

    let id = added["id"].as_str().unwrap();
    let run = call(&host, "job_run", json!({"id": id})).await;
    assert_eq!(run["run"]["outcome"], "OUTCOME_RUNNING");
    // The run is done in the background: wait for its end.
    let mut ended = Value::Null;
    for _ in 0..100 {
        let runs = call(&host, "run_list", json!({"job": id})).await;
        if runs["runs"][0]["run"]["outcome"] != "OUTCOME_RUNNING" {
            ended = runs["runs"][0]["run"].clone();
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(ended["outcome"], "OUTCOME_OK", "{ended}");
    let tasks = call(
        &host,
        "task_list",
        json!({"workspace_id": ws, "board": board_id}),
    )
    .await;
    let tasks = tasks["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0]["id"], ended["result"], "the run names its task");
    assert_eq!(tasks[0]["task"]["assignee"], "eq-sentinel");
    assert_eq!(tasks[0]["author"], "scheduler");

    let listed = call(&host, "job_list", json!({})).await;
    assert_eq!(
        listed["jobs"][0]["last_run"]["run"]["outcome"],
        "OUTCOME_OK"
    );

    // Watched from outside: a probe that carries no token reads whether a
    // job of a name runs on time.
    let probe = |name: &'static str| {
        let routes = post_cortex_transport::job_health::routes(host.channel(Origin::Client));
        let request = Request::get(format!("/health/jobs/{name}"))
            .body(Body::empty())
            .unwrap();
        async move { routes.oneshot(request).await.unwrap().status() }
    };
    assert_eq!(probe("daily-report").await, StatusCode::OK);
    assert_eq!(probe("no-such-job").await, StatusCode::NOT_FOUND);
    let mut paused = listed["jobs"][0].clone();
    paused["job"]["paused"] = json!(true);
    call(
        &host,
        "job_update",
        json!({"id": id, "version": paused["version"], "job": paused["job"]}),
    )
    .await;
    assert_eq!(
        probe("daily-report").await,
        StatusCode::SERVICE_UNAVAILABLE,
        "a paused job does not run"
    );
}
