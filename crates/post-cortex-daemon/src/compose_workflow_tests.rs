// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A workflow's `agent` block (docs/modules/workflow.md, blocks): a team
//! member's turn is the team's, so the member's role and modules apply, as
//! on the board, and a person is asked for it by name; the agent's own has
//! the tools of the modules the workflow's grants name. Only the model is
//! scripted.

use std::sync::Arc;

use pcx_host::Host;
use pcx_module_api::model::Block;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::compose::{Builtin, Modules, feed_memory_writes, host};
use crate::compose_dynamic_agent_tests::{builtin, person, tools_of, until};
use crate::compose_dynamic_stack_tests::memory_up;
use crate::compose_dynamic_tests::approve;
use crate::compose_scripted_tests::{Provider, Scripted};

/// Workflow `name` of one `agent` block (`look`) with `grants`, added,
/// approved by a person and run to its end: what it asked a person for, and
/// the run.
async fn ran(
    host: &Host,
    workspace: &str,
    name: &str,
    look: Value,
    grants: &[&str],
) -> (Value, Value) {
    let created = person(
        host,
        "workflow_create",
        json!({ "workflow": {
            "name": name,
            "input": { "fields": [] },
            "graph": {
                "blocks": [{ "id": "start", "trigger": { "manual": {} } }, { "id": "look", "agent": look }],
                "edges": [{ "from": "start", "to": "look" }],
            },
            "output": "steps.look.report",
            "budgets": { "steps": 10, "wall": "60s" },
            "grants": { "modules": grants },
        } }),
    )
    .await;
    let asked = created["workflow"]["asked"].clone();
    approve(host, workspace, &asked).await;
    let id = created["id"].as_str().unwrap().to_string();
    until("the workflow was approved", async || {
        let shown = person(host, "workflow_list", json!({})).await;
        shown["workflows"]
            .as_array()?
            .iter()
            .find(|w| w["id"] == json!(id))?["workflow"]
            .get("approved")
            .map(|_| ())
    })
    .await;
    let started = person(host, "workflow_run", json!({ "workflow": name })).await;
    let run = started["id"].as_str().unwrap().to_string();
    let detail = until("the run ended", async || {
        let detail = person(host, "workflow_run_get", json!({ "id": run })).await;
        let state = detail["run"]["run"]["status"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        (!state.is_empty() && !state.contains("RUNNING") && !state.contains("PENDING"))
            .then_some(detail)
    })
    .await;
    (asked, detail)
}

/// Ops daemon 2026-11-11: `eq-sentinel`, a member of `observe`, `board` and
/// memory only, reached `workflow_run` in the workflow `eq-daily`: its turn
/// went to the agent, which reads no member.
#[tokio::test(flavor = "multi_thread")]
async fn a_workflows_turn_has_its_members_tools_or_those_of_its_grants() {
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
        vec![Block::Text {
            text: r#"{"report": "Nothing fires."}"#.into(),
        }],
        vec![Block::Text {
            text: r#"{"report": "Still nothing."}"#.into(),
        }],
    ]);
    let specs = [
        builtin(Builtin::Board, None),
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
        .unwrap();
    let running = Arc::new(host(&modules).await.unwrap());
    let stop = CancellationToken::new();
    let feed = tokio::spawn(feed_memory_writes(
        Arc::clone(&running),
        Arc::clone(&memory),
        stop.clone(),
    ));

    // A person who approves, and a member that may use the board only.
    person(&running, "agent_add", json!({ "workspace_id": workspace, "agent": { "name": "ana", "role": "owner", "authority": "AUTHORITY_CLOSE", "harness": false } })).await;
    person(&running, "agent_add", json!({ "workspace_id": workspace, "agent": { "name": "eq-sentinel", "role": "on-call, reads only", "authority": "AUTHORITY_DISCUSS", "harness": true, "modules": ["board"] } })).await;
    person(
        &running,
        "team_create",
        json!({ "workspace_id": workspace, "name": "eq-ops", "members": ["ana", "eq-sentinel"] }),
    )
    .await;

    // A member's turn: asked for by name, then its role in the
    // instructions, the board's tools its modules name and no other
    // module's (the workflow's own least).
    let look = json!({ "member": "eq-sentinel", "prompt": "Is anything firing?",
        "output": { "fields": [{ "name": "report", "type": "string", "required": true }] } });
    let (asked, detail) = ran(&running, &workspace, "daily", look, &[]).await;
    assert_eq!(
        asked["grants"]["members"],
        json!(["eq-sentinel"]),
        "{asked}"
    );
    assert!(detail.to_string().contains("Nothing fires."), "{detail}");
    let turn = scripted.asked.lock()[0].clone();
    let tools = tools_of(&turn);
    assert!(
        turn.system.contains("on-call, reads only"),
        "{}",
        turn.system
    );
    assert!(tools.iter().any(|t| t == "task_get"), "{tools:?}");
    assert!(
        !tools.iter().any(|t| t.starts_with("workflow_")),
        "{tools:?}"
    );

    // The agent's own turn: the tools of the modules the grants name.
    let own = json!({ "prompt": "Is anything firing?",
        "output": { "fields": [{ "name": "report", "type": "string", "required": true }] } });
    let (asked, detail) = ran(&running, &workspace, "own", own, &["board"]).await;
    assert_eq!(asked["grants"]["members"], json!([]), "{asked}");
    assert!(detail.to_string().contains("Still nothing."), "{detail}");
    let turn = scripted.asked.lock()[1].clone();
    let tools = tools_of(&turn);
    assert!(
        !turn.system.contains("on-call, reads only"),
        "{}",
        turn.system
    );
    assert!(tools.iter().any(|t| t == "task_get"), "{tools:?}");
    assert!(
        !tools.iter().any(|t| t.starts_with("workflow_")),
        "{tools:?}"
    );

    stop.cancel();
    feed.await.unwrap();
    running.stop().await;
}
