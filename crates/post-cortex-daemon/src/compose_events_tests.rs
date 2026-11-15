// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The organisation's bus as the daemon composes it (docs/host.md, events
//! of a module): the board's events and memory's, kept by memory through
//! the host.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use pcx_board::BoardModule;
use pcx_host::{EventStore, Host};
use pcx_module_api::{Module, Scope};
use post_cortex_memory::capabilities::CreateWorkspace;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::compose::MemoryEvents;

fn scope() -> Scope {
    Scope {
        tenant: String::new(),
        profile: String::new(),
        session_id: None,
        workspace_id: None,
        author: Some("agent".to_string()),
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
async fn a_task_a_decision_and_its_record_are_events_of_the_bus() {
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
    let modules: Vec<Arc<dyn Module>> = vec![
        crate::memory_tests::memory_module(&memory).0,
        Arc::new(BoardModule::new()),
    ];
    let store = Arc::new(MemoryEvents::new(&memory));
    let host = Host::start_with(
        modules,
        &HashMap::new(),
        &HashMap::new(),
        Some(store.clone() as Arc<dyn EventStore>),
        "",
    )
    .await
    .unwrap();

    let board = call(
        &host,
        "board_create",
        json!({"workspace_id": ws, "name": "Sprint", "columns": ["todo", "done"]}),
    )
    .await;
    let task = call(
        &host,
        "task_add",
        json!({"workspace_id": ws, "board": board["id"], "title": "Ship"}),
    )
    .await;
    call(
        &host,
        "decide",
        json!({"workspace_id": ws, "task": task["id"], "what": "ship", "why": "ready", "risk": "RISK_NONE"}),
    )
    .await;

    let created = store.read("probe", "board.task_created", 10).await.unwrap();
    assert_eq!(created.events.len(), 1);
    assert_eq!(created.events[0].workspace_id, ws);
    assert_eq!(created.events[0].author, "agent");
    let decided = store
        .read("probe", "board.decision_made", 10)
        .await
        .unwrap();
    assert_eq!(decided.events.len(), 1);

    // The decision's record is announced once memory has written it.
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        let added = store
            .read("probe", "post-cortex.memory.record_added", 10)
            .await
            .unwrap();
        if let Some(event) = added.events.first() {
            assert_eq!(event.module, "post-cortex.memory");
            assert_eq!(event.workspace_id, ws);
            break;
        }
        assert!(Instant::now() < until, "no record was announced");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
