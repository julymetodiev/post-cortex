// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A team that starts goes on with the work a stop cut short (docs/connect.md
//! section 7), over the real host, memory and board: what it reads to find
//! that work is what the host lets a module call.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use pcx_agent::AgentModule;
use pcx_board::BoardModule;
use pcx_host::Host;
use pcx_module_api::Module;
use pcx_module_api::model::Block;
use pcx_skills::SkillsModule;
use pcx_team::TeamModule;
use post_cortex_memory::Memory;
use post_cortex_memory::capabilities::CreateWorkspace;
use serde_json::json;

use crate::compose_scripted_tests::{Provider, Scripted};
use crate::compose_team_tests::call;

async fn until(what: &str, holds: impl AsyncFn() -> bool) {
    let started = Instant::now();
    while !holds().await {
        assert!(started.elapsed() < Duration::from_secs(20), "never: {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn host(memory: &Arc<Memory>, scripted: &Arc<Scripted>, ws: &str) -> Arc<Host> {
    let modules: Vec<Arc<dyn Module>> = vec![
        crate::memory_tests::memory_module(memory).0,
        Arc::new(Provider(Arc::clone(scripted))),
        Arc::new(BoardModule::new()),
        Arc::new(SkillsModule::new()),
        Arc::new(AgentModule::new()),
        Arc::new(TeamModule::new()),
    ];
    let config = HashMap::from([(SkillsModule::ID.to_string(), json!({ "workspace_id": ws }))]);
    Arc::new(
        Host::start(modules, &config, &HashMap::new())
            .await
            .unwrap(),
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn a_team_that_starts_goes_on_with_the_work_a_stop_cut_short() {
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

    // Its one answer never comes: the stop cuts the turn short.
    let first = Scripted::with(vec![Vec::new()]);
    let before = host(&memory, &first, &ws).await;
    let stop = tokio_util::sync::CancellationToken::new();
    let feed = tokio::spawn(crate::compose::feed_memory_writes(
        Arc::clone(&before),
        Arc::clone(&memory),
        stop.clone(),
    ));
    call(&before, "julius", "agent_add", json!({ "workspace_id": ws, "agent": { "name": "julius", "role": "owner", "authority": "AUTHORITY_CLOSE", "harness": false } })).await;
    call(&before, "julius", "agent_add", json!({ "workspace_id": ws, "agent": { "name": "dev", "role": "writes code", "authority": "AUTHORITY_DISCUSS", "harness": true } })).await;
    call(
        &before,
        "julius",
        "team_create",
        json!({ "workspace_id": ws, "name": "core", "members": ["julius", "dev"] }),
    )
    .await;
    let board = call(
        &before,
        "julius",
        "board_create",
        json!({ "workspace_id": ws, "name": "Sprint", "columns": ["todo", "done"] }),
    )
    .await;
    let task = call(
        &before,
        "julius",
        "task_add",
        json!({ "workspace_id": ws, "board": board["id"], "title": "Ship", "assignee": "dev" }),
    )
    .await;
    let id = task["id"].as_str().unwrap().to_string();
    until("it asks its model", async || !first.asked.lock().is_empty()).await;
    stop.cancel();
    feed.await.unwrap();
    before.stop().await;

    let second = Scripted::with(vec![vec![Block::Text {
        text: "Went on.".into(),
    }]]);
    let after = host(&memory, &second, &ws).await;
    let stop = tokio_util::sync::CancellationToken::new();
    let feed = tokio::spawn(crate::compose::feed_memory_writes(
        Arc::clone(&after),
        Arc::clone(&memory),
        stop.clone(),
    ));
    until(
        "it goes on with no call and says so on its task",
        async || {
            let thread = call(
                &after,
                "julius",
                "task_get",
                json!({ "workspace_id": ws, "task": id }),
            )
            .await;
            thread["comments"]
                .as_array()
                .is_some_and(|c| c.iter().any(|c| c["text"] == json!("Went on.")))
        },
    )
    .await;
    let asked = format!("{:?}", second.asked.lock()[0]);
    assert!(asked.contains("restart"), "it is told why: {asked}");

    stop.cancel();
    feed.await.unwrap();
    after.stop().await;
}
