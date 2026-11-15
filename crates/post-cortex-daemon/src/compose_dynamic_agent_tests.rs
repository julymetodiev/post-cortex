// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A dynamic module's way through an agent, as it is meant to be used
//! (docs/modules/dynamic.md): a task on the board goes to `module-builder`,
//! whose turn installs the module; a person approves it; a turn calls it.
//! Only the model is scripted.
//!
//! ```sh
//! PCX_BINDINGS_PYTHON=bindings/python/.venv/bin/python PCX_BINDINGS_DENO=deno \
//! scripts/cargo.sh ignored -E 'test(an_agent_builds_a_module)'
//! ```

use std::sync::Arc;
use std::time::{Duration, Instant};

use pcx_host::Host;
use pcx_module_api::model::{Block, ModelRequest};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::compose::{Builtin, BuiltinSpec, Modules, feed_memory_writes, host, keep_dynamic};
use crate::compose_dynamic_stack_tests::{launcher_up, memory_up};
use crate::compose_dynamic_tests::{approve, install, scope};
use crate::compose_scripted_tests::{Provider, Scripted};

/// The built-in `id` with `config`, no grants.
pub fn builtin(id: Builtin, config: Option<Value>) -> BuiltinSpec {
    BuiltinSpec {
        id,
        grants: Vec::new(),
        filesystem: Vec::new(),
        config,
    }
}

/// The data of `tool` with `input`, called by the person `ana`.
pub async fn person(host: &Host, tool: &str, input: Value) -> Value {
    crate::memory_tests::as_person(host, &scope("ana"), tool, input)
        .await
        .unwrap()
}

fn tool_use(id: &str, name: &str, input: Value) -> Block {
    Block::ToolUse {
        id: id.to_string(),
        name: name.to_string(),
        input,
    }
}

/// The names of the tools `request` gave its model.
pub fn tools_of(request: &ModelRequest) -> Vec<String> {
    request.tools.iter().map(|t| t.name.clone()).collect()
}

/// `read` once it answers, within a while.
pub async fn until<T>(what: &str, read: impl AsyncFn() -> Option<T>) -> T {
    let started = Instant::now();
    loop {
        if let Some(found) = read().await {
            return found;
        }
        assert!(started.elapsed() < Duration::from_secs(60), "never: {what}");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the built Python binding: set PCX_BINDINGS_PYTHON and pass --ignored"]
async fn an_agent_builds_a_module_a_person_approves_and_agents_call_it() {
    let stack = launcher_up();
    let (_data, memory, workspace) = memory_up().await;
    let boards = [builtin(Builtin::Board, None)];
    let first = Modules::memory(Arc::clone(&memory))
        .unwrap()
        .configured(&[], &[], &boards)
        .await
        .unwrap();
    let first = host(&first).await.unwrap();
    let board = person(
        &first,
        "board_create",
        json!({ "workspace_id": workspace, "name": "Approvals", "columns": ["todo", "done"] }),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_string();
    first.stop().await;

    let scripted = Scripted::with(vec![
        vec![tool_use("c1", "dynamic_install", install("Hello"))],
        vec![Block::Text {
            text: "Installed; its decision waits on the board.".into(),
        }],
        vec![tool_use("c2", "hello_say", json!({ "text": "Ada" }))],
        vec![Block::Text {
            text: "It answers.".into(),
        }],
    ]);
    let specs = [
        builtin(Builtin::Board, None),
        builtin(Builtin::Skills, Some(json!({ "workspace_id": workspace }))),
        builtin(Builtin::Agent, None),
        builtin(Builtin::Team, None),
        builtin(
            Builtin::Workflow,
            Some(json!({ "workspace_id": workspace, "board": board })),
        ),
    ];
    let modules = Modules::memory(Arc::clone(&memory))
        .unwrap()
        .with_remote(vec![Arc::new(Provider(Arc::clone(&scripted)))])
        .configured(&[], &[], &specs)
        .await
        .unwrap()
        .with_dynamic(
            stack.launcher.clone(),
            json!({ "workspace_id": workspace, "board": board }),
        );
    let running = Arc::new(host(&modules).await.unwrap());
    let stop = CancellationToken::new();
    let feed = tokio::spawn(feed_memory_writes(
        Arc::clone(&running),
        Arc::clone(&memory),
        stop.clone(),
    ));
    let keeper = tokio::spawn(keep_dynamic(
        Arc::clone(&running),
        stack.launcher.clone(),
        "org-1".to_string(),
        stop.clone(),
    ));

    // A person, and the built-in agent in a team.
    person(&running, "agent_add", json!({ "workspace_id": workspace, "agent": { "name": "ana", "role": "owner", "authority": "AUTHORITY_CLOSE", "harness": false } })).await;
    let team = person(
        &running,
        "team_create",
        json!({ "workspace_id": workspace, "name": "core", "members": ["ana"] }),
    )
    .await;
    person(
        &running,
        "member_add",
        json!({ "workspace_id": workspace, "team": "core", "member": "module-builder", "version": team["version"] }),
    )
    .await;

    // A task on the board: its turn has the tool and installs.
    let task = person(
        &running,
        "task_add",
        json!({ "workspace_id": workspace, "board": board, "title": "A module that says hello", "assignee": "module-builder" }),
    )
    .await;
    let asked = until("the module was asked about", async || {
        let shown = running
            .call_for_agent(&scope("tester"), "dynamic_get", json!({ "id": "hello" }))
            .await
            .ok()?
            .data?;
        shown["module"].get("asked").cloned()
    })
    .await;
    let first_turn = scripted.asked.lock()[0].clone();
    assert!(
        tools_of(&first_turn).iter().any(|t| t == "dynamic_install"),
        "{:?}",
        tools_of(&first_turn)
    );
    assert!(
        first_turn.system.contains("module-builder"),
        "{}",
        first_turn.system
    );
    assert!(!tools_of(&first_turn).iter().any(|t| t == "hello_say"));

    // Approved by a person: it runs.
    approve(&running, &workspace, &asked).await;
    until("the module runs", async || {
        running
            .tools()
            .iter()
            .any(|t| t.name == "hello_say")
            .then_some(())
    })
    .await;

    // A later turn has its tool and gets its answer.
    person(
        &running,
        "comment",
        json!({ "workspace_id": workspace, "task": task["id"], "text": "@module-builder call it" }),
    )
    .await;
    until("the second turn ended", async || {
        (scripted.asked.lock().len() >= 4).then_some(())
    })
    .await;
    let requests = scripted.asked.lock().clone();
    assert!(
        tools_of(&requests[2]).iter().any(|t| t == "hello_say"),
        "{:?}",
        tools_of(&requests[2])
    );
    let result = format!("{:?}", requests[3].messages);
    assert!(result.contains("Hello Ada"), "{result}");

    // A workflow calls it with a `tool` block, as any module's.
    let created = person(
        &running,
        "workflow_create",
        json!({ "workflow": {
            "name": "says-hello",
            "input": { "fields": [] },
            "graph": {
                "blocks": [{ "id": "start", "trigger": { "manual": {} } }, { "id": "say", "tool": {
                    "method": "/hello.v1.Hello/Say",
                    "request": { "fields": { "text": { "constant": { "string": "Ada" } } } },
                } }],
                "edges": [{ "from": "start", "to": "say" }],
            },
            "output": "steps.say.text",
            "budgets": { "steps": 10, "wall": "60s" },
            "grants": { "modules": ["hello"] },
        } }),
    )
    .await;
    let approval = created["workflow"]["asked"].clone();
    approve(&running, &workspace, &approval).await;
    let id = created["id"].as_str().unwrap().to_string();
    until("the workflow was approved", async || {
        let shown = person(&running, "workflow_list", json!({})).await;
        shown["workflows"]
            .as_array()?
            .iter()
            .find(|w| w["id"] == json!(id))?["workflow"]
            .get("approved")
            .map(|_| ())
    })
    .await;
    let started = person(
        &running,
        "workflow_run",
        json!({ "workflow": "says-hello" }),
    )
    .await;
    let run = started["id"].as_str().unwrap().to_string();
    let detail = until("the run ended", async || {
        let detail = person(&running, "workflow_run_get", json!({ "id": run })).await;
        let state = detail["run"]["run"]["status"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        (!state.is_empty() && !state.contains("RUNNING") && !state.contains("PENDING"))
            .then_some(detail)
    })
    .await;
    assert!(detail.to_string().contains("Hello Ada"), "{detail}");

    stop.cancel();
    feed.await.unwrap();
    keeper.await.unwrap();
    running.stop().await;
    stack.stop.cancel();
    stack.task.await.unwrap().unwrap();
}
