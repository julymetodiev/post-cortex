// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The board over memory as the daemon composes it (ARCHITECTURE section
//! 8): its documents in memory, its view checked at load, a decision
//! recorded in the board's session and holding its task until a person
//! approves it.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use pcx_board::BoardModule;
use pcx_host::Host;
use pcx_module_api::{Module, ModuleErrorKind, Scope};
use post_cortex_memory::capabilities::CreateWorkspace;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

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
        deadline: Instant::now() + Duration::from_secs(10),
        cancel: CancellationToken::new(),
    }
}

async fn call(host: &Host, author: &str, tool: &str, input: Value) -> Value {
    host.call_for_agent(&scope(author), tool, input)
        .await
        .unwrap()
        .data
        .unwrap()
}

/// Memory in `data` with one workspace and the board over it, started.
async fn started(data: &tempfile::TempDir) -> (Arc<post_cortex_memory::Memory>, uuid::Uuid, Host) {
    let memory = Arc::new(
        crate::memory_tests::rocksdb_memory(
            data,
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
        .id;
    let modules: Vec<Arc<dyn Module>> = vec![
        crate::memory_tests::memory_module(&memory).0,
        Arc::new(BoardModule::new()),
    ];
    // The start checks the board's document kinds and its view.
    let host = Host::start(modules, &HashMap::new(), &HashMap::new())
        .await
        .unwrap();
    (memory, ws, host)
}

#[tokio::test]
async fn a_board_keeps_its_tasks_and_decisions_in_memory() {
    let data = tempfile::tempdir().unwrap();
    let (memory, ws, host) = started(&data).await;
    let w = ws.to_string();

    let board = call(
        &host,
        "agent",
        "board_create",
        json!({"workspace_id": w, "name": "Sprint", "columns": ["todo", "doing", "done"]}),
    )
    .await;
    let session = board["board"]["session_id"].as_str().unwrap().to_string();
    let joined = memory.workspaces.get(ws).unwrap();
    assert!(
        joined.get_session_role(&session.parse().unwrap()).is_some(),
        "the board's session is in the workspace"
    );

    let task = call(
        &host,
        "agent",
        "task_add",
        json!({"workspace_id": w, "board": board["id"], "title": "Drop the old table"}),
    )
    .await;
    assert_eq!(task["task"]["column"], json!("todo"));

    let decision = call(
        &host,
        "agent",
        "decide",
        json!({"workspace_id": w, "task": task["id"], "what": "drop it", "why": "unused", "risk": "RISK_DATA"}),
    )
    .await;
    assert_eq!(decision["pending"], json!(true));
    let recorded = memory.sessions.get(session.parse().unwrap()).await.unwrap();
    assert_eq!(
        recorded.records().len(),
        1,
        "the decision is a record of the board's session"
    );

    let held = host
        .call_for_agent(
            &scope("agent"),
            "task_move",
            json!({"workspace_id": w, "task": task["id"], "version": task["version"], "column": "done"}),
        )
        .await
        .unwrap_err();
    assert_eq!(held.kind, ModuleErrorKind::FailedPrecondition);

    let approve =
        json!({"workspace_id": w, "decision": decision["id"], "version": decision["version"]});
    let by_agent = host
        .call_for_agent(&scope("agent"), "approve_decision", approve.clone())
        .await
        .unwrap_err();
    assert_eq!(by_agent.kind, ModuleErrorKind::Denied, "{by_agent}");
    let approved =
        crate::memory_tests::as_person(&host, &scope("julius"), "approve_decision", approve)
            .await
            .unwrap();
    assert_eq!(approved["decision"]["approved_by"], json!("julius"));

    let moved = call(
        &host,
        "agent",
        "task_move",
        json!({"workspace_id": w, "task": task["id"], "version": task["version"], "column": "done"}),
    )
    .await;
    assert_eq!(moved["task"]["column"], json!("done"));
    let thread = call(
        &host,
        "agent",
        "task_get",
        json!({"workspace_id": w, "task": task["id"]}),
    )
    .await;
    assert_eq!(thread["decisions"][0]["pending"], json!(false));
    host.stop().await;
}

#[tokio::test]
async fn a_task_is_named_by_its_short_id_over_memory() {
    let data = tempfile::tempdir().unwrap();
    let (_memory, ws, host) = started(&data).await;
    let w = ws.to_string();
    let board = call(
        &host,
        "agent",
        "board_create",
        json!({"workspace_id": w, "name": "Sprint", "columns": ["todo", "doing", "done"]}),
    )
    .await;
    let task = call(
        &host,
        "agent",
        "task_add",
        json!({"workspace_id": w, "board": board["id"], "title": "Ship"}),
    )
    .await;
    let id = task["id"].as_str().unwrap();
    // Memory refuses a name that is not a whole id; the board reads it as
    // the start of one.
    let short = &id[..8];
    let moved = call(
        &host,
        "agent",
        "task_move",
        json!({"workspace_id": w, "task": short, "version": task["version"], "column": "doing"}),
    )
    .await;
    assert_eq!(moved["id"], json!(id));
    assert_eq!(moved["task"]["column"], json!("doing"));
    let got = call(
        &host,
        "agent",
        "task_get",
        json!({"workspace_id": w, "task": short}),
    )
    .await;
    assert_eq!(got["task"]["id"], json!(id));

    let unknown = host
        .call_for_agent(
            &scope("agent"),
            "task_get",
            json!({"workspace_id": w, "task": "ffffffff"}),
        )
        .await
        .unwrap_err();
    assert_eq!(unknown.kind, ModuleErrorKind::NotFound, "{unknown}");
}
