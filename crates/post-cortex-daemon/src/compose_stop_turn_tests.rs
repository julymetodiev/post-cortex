// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A person stops a member's running turn: it ends at once, tries no more and
//! says so on its task.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use pcx_agent::AgentModule;
use pcx_board::BoardModule;
use pcx_host::Host;
use pcx_module_api::Module;
use pcx_skills::SkillsModule;
use pcx_team::TeamModule;
use post_cortex_memory::capabilities::CreateWorkspace;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::compose_scripted_tests::{Provider, Scripted};
use crate::compose_team_tests::call;

async fn until(what: &str, holds: impl AsyncFn() -> bool) {
    let started = Instant::now();
    while !holds().await {
        assert!(started.elapsed() < Duration::from_secs(20), "never: {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_person_stops_a_members_running_turn() {
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
    // Its one answer never comes.
    let scripted = Scripted::with(vec![Vec::new()]);
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
        json!({ "workspace_id": ws, "board": board["id"], "title": "Ship", "assignee": "dev" }),
    )
    .await;
    let id = task["id"].as_str().unwrap().to_string();
    let busy = async || {
        let work = call(&host, "julius", "work_list", json!({ "workspace_id": ws })).await;
        work["work"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["task"] == json!(id) && w["busy"] == json!(true))
    };
    until("its turn runs", busy).await;
    until("it asks its model", async || {
        !scripted.asked.lock().is_empty()
    })
    .await;

    let stopped = call(
        &host,
        "julius",
        "work_stop",
        json!({ "workspace_id": ws, "task": id, "member": "dev" }),
    )
    .await;
    assert_eq!(stopped["stopped"], json!(true));
    until("its turn ended", async || !busy().await).await;
    until("the task says so", async || {
        let thread = call(
            &host,
            "julius",
            "task_get",
            json!({ "workspace_id": ws, "task": id }),
        )
        .await;
        thread["comments"].as_array().is_some_and(|c| {
            c.iter().any(|c| {
                c["text"]
                    .as_str()
                    .is_some_and(|t| t.starts_with("Stopped:"))
            })
        })
    })
    .await;
    // No other try, and nothing to stop any more.
    assert_eq!(scripted.asked.lock().len(), 1);
    let again = call(
        &host,
        "julius",
        "work_stop",
        json!({ "workspace_id": ws, "task": id, "member": "dev" }),
    )
    .await;
    assert!(again.get("stopped").is_none_or(|s| s == &json!(false)));
    let thread = call(
        &host,
        "julius",
        "task_get",
        json!({ "workspace_id": ws, "task": id }),
    )
    .await;
    assert!(
        thread["task"]["task"]
            .get("labels")
            .is_none_or(|l| !l.to_string().contains("blocked"))
    );

    stop.cancel();
    feed.await.unwrap();
    host.stop().await;
}
