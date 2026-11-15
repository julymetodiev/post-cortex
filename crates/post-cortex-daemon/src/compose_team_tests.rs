// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The team over the board, the agent and memory as the daemon composes
//! them (ARCHITECTURE section 8): a person mentions a member of the
//! harness in a comment, the member works on the task in its own session
//! and answers in the thread; the board checks the team's authority.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use pcx_agent::AgentModule;
use pcx_board::BoardModule;
use pcx_host::Host;
use pcx_module_api::model::Block;
use pcx_module_api::{Module, ModuleErrorKind, Scope};
use pcx_observe::ObserveModule;
use pcx_ops::OpsModule;
use pcx_skills::SkillsModule;
use pcx_team::TeamModule;
use post_cortex_memory::capabilities::CreateWorkspace;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::compose_scripted_tests::{Provider, Scripted};

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

/// The data of `tool` with `input`, called by `author` as a person calls
/// it (the app).
pub async fn call(host: &Host, author: &str, tool: &str, input: Value) -> Value {
    crate::memory_tests::as_person(host, &scope(author), tool, input)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_mentioned_member_answers_in_the_thread() {
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
    let scripted = Scripted::with(vec![vec![Block::Text {
        text: "I will start with the release notes.".into(),
    }]]);
    let modules: Vec<Arc<dyn Module>> = vec![
        crate::memory_tests::memory_module(&memory).0,
        Arc::new(Provider(Arc::clone(&scripted))),
        Arc::new(BoardModule::new()),
        Arc::new(SkillsModule::new()),
        Arc::new(AgentModule::new()),
        Arc::new(TeamModule::new()),
    ];
    let config = HashMap::from([(SkillsModule::ID.to_string(), json!({ "workspace_id": ws }))]);
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

    call(&host, "julius", "agent_add", json!({ "workspace_id": ws, "agent": { "name": "julius", "role": "owner", "authority": "AUTHORITY_CLOSE", "harness": false } })).await;
    call(&host, "julius", "agent_add", json!({ "workspace_id": ws, "agent": { "name": "dev", "role": "writes code", "authority": "AUTHORITY_DISCUSS", "harness": true } })).await;
    call(
        &host,
        "julius",
        "team_create",
        json!({ "workspace_id": ws, "name": "core", "members": ["julius", "dev"] }),
    )
    .await;
    let board = call(
        &host,
        "julius",
        "board_create",
        json!({ "workspace_id": ws, "name": "Sprint", "columns": ["todo", "done"] }),
    )
    .await;
    let task = call(
        &host,
        "julius",
        "task_add",
        json!({ "workspace_id": ws, "board": board["id"], "title": "Ship" }),
    )
    .await;
    call(
        &host,
        "julius",
        "comment",
        json!({ "workspace_id": ws, "task": task["id"], "text": "@dev can you start?" }),
    )
    .await;

    // The member's answer comes as a comment in the thread.
    let mut answered = Value::Null;
    for _ in 0..100 {
        let thread = call(
            &host,
            "julius",
            "task_get",
            json!({ "workspace_id": ws, "task": task["id"] }),
        )
        .await;
        if let Some(c) = thread["comments"]
            .as_array()
            .and_then(|c| c.iter().find(|c| c["author"] == json!("dev")))
        {
            answered = c.clone();
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        answered["text"],
        json!("I will start with the release notes.")
    );
    // It was told its role and the thread.
    let asked = scripted.asked.lock()[0].clone();
    assert!(asked.system.contains("You are dev"), "{}", asked.system);

    // The board checks the team: dev may only discuss.
    let refused = host
        .call_for_agent(
            &scope("dev"),
            "decide",
            json!({ "workspace_id": ws, "task": task["id"], "what": "ship", "why": "ready", "risk": "RISK_NONE" }),
        )
        .await
        .unwrap_err();
    assert_eq!(refused.kind, ModuleErrorKind::Denied);
    // Nor may an agent raise its own authority: the team's tools that say
    // who may do what are a person's.
    let raised = host
        .call_for_agent(
            &scope("dev"),
            "agent_update",
            json!({ "workspace_id": ws, "name": "dev", "version": 1, "authority": "AUTHORITY_CLOSE" }),
        )
        .await
        .unwrap_err();
    assert_eq!(raised.kind, ModuleErrorKind::Denied);
    stop.cancel();
    feed.await.unwrap();
}

/// W32: a member's work whose model refuses the request as it is blocks its
/// task at once, another try would end the same (owner 2026-10-29): the
/// label `blocked` and a comment why, though the settings give two tries.
#[tokio::test]
async fn work_whose_model_refuses_it_blocks_its_task_at_once() {
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
    // No script: every call of the model fails as invalid.
    let scripted = Scripted::with(Vec::new());
    let modules: Vec<Arc<dyn Module>> = vec![
        crate::memory_tests::memory_module(&memory).0,
        Arc::new(Provider(Arc::clone(&scripted))),
        Arc::new(BoardModule::new()),
        Arc::new(SkillsModule::new()),
        Arc::new(AgentModule::new()),
        Arc::new(TeamModule::new()),
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
    call(&host, "julius", "agent_add", json!({ "workspace_id": ws, "agent": { "name": "julius", "role": "owner", "authority": "AUTHORITY_CLOSE", "harness": false } })).await;
    call(&host, "julius", "agent_add", json!({ "workspace_id": ws, "agent": { "name": "dev", "role": "writes code", "authority": "AUTHORITY_DISCUSS", "harness": true } })).await;
    call(
        &host,
        "julius",
        "team_create",
        json!({ "workspace_id": ws, "name": "core", "members": ["julius", "dev"] }),
    )
    .await;
    let board = call(
        &host,
        "julius",
        "board_create",
        json!({ "workspace_id": ws, "name": "Sprint", "columns": ["todo", "done"] }),
    )
    .await;
    let task = call(
        &host,
        "julius",
        "task_add",
        json!({ "workspace_id": ws, "board": board["id"], "title": "Ship", "assignee": "dev" }),
    )
    .await;

    let mut thread = Value::Null;
    for _ in 0..200 {
        thread = call(
            &host,
            "julius",
            "task_get",
            json!({ "workspace_id": ws, "task": task["id"] }),
        )
        .await;
        // The team labels the task, then says why on it: two writes, so
        // both are waited for.
        let labels = &thread["task"]["task"]["labels"];
        let blocked = labels
            .as_array()
            .is_some_and(|l| l.iter().any(|x| x == "blocked"));
        if blocked && thread["comments"][0]["text"].is_string() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        thread["task"]["task"]["labels"],
        json!(["blocked"]),
        "{thread}"
    );
    let said = thread["comments"][0]["text"].as_str().unwrap();
    assert!(
        said.starts_with("Blocked: dev's work failed once it was tried (1)"),
        "{said}"
    );
    assert!(said.contains("invalid: no more script"), "{said}");
    assert_eq!(
        thread["comments"][0]["author"], "team",
        "the team blocked it"
    );
    stop.cancel();
    feed.await.unwrap();
}

/// A member that may only read (ARCHITECTURE section 8, observe): its turn is
/// given the tools of the modules it names, observe's among them, and not
/// ops', though ops runs.
#[tokio::test]
async fn a_member_that_may_only_read_is_given_observe_and_not_ops() {
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
            name: "ops".into(),
            description: String::new(),
        })
        .await
        .unwrap()
        .id
        .to_string();
    let scripted = Scripted::with(vec![vec![Block::Text {
        text: "Nothing fires.".into(),
    }]]);
    let modules: Vec<Arc<dyn Module>> = vec![
        crate::memory_tests::memory_module(&memory).0,
        Arc::new(Provider(Arc::clone(&scripted))),
        Arc::new(BoardModule::new()),
        Arc::new(AgentModule::new()),
        Arc::new(TeamModule::new()),
        Arc::new(ObserveModule::new()),
        Arc::new(OpsModule::new()),
    ];
    let config = HashMap::from([(OpsModule::ID.to_string(), json!({ "workspace_id": ws }))]);
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
    call(&host, "julius", "agent_add", json!({ "workspace_id": ws, "agent": { "name": "eq-sentinel", "role": "on-call, reads only", "authority": "AUTHORITY_DISCUSS", "harness": true, "modules": ["observe", "board"] } })).await;
    call(
        &host,
        "julius",
        "team_create",
        json!({ "workspace_id": ws, "name": "eq-ops", "members": ["eq-sentinel"] }),
    )
    .await;
    let board = call(
        &host,
        "julius",
        "board_create",
        json!({ "workspace_id": ws, "name": "eq-ops", "columns": ["inbox", "done"] }),
    )
    .await;
    call(
        &host,
        "julius",
        "task_add",
        json!({ "workspace_id": ws, "board": board["id"], "title": "Is anything firing?", "assignee": "eq-sentinel" }),
    )
    .await;

    for _ in 0..100 {
        if !scripted.asked.lock().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let tools: Vec<String> = scripted.asked.lock()[0]
        .tools
        .iter()
        .map(|t| t.name.clone())
        .collect();
    assert!(tools.iter().any(|t| t == "metrics_query"), "{tools:?}");
    assert!(tools.iter().any(|t| t == "task_get"), "{tools:?}");
    assert!(!tools.iter().any(|t| t.starts_with("ops_")), "{tools:?}");
    stop.cancel();
    feed.await.unwrap();
}
