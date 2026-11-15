// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! One engine at work in several workspaces (docs/modules/workflow.md, its
//! workspaces): runs of different workspaces go at once, each kept in its
//! own, and an event starts runs only in the workspace it is of.

use std::collections::HashMap;

use post_cortex_proto::workflow::block;

use super::*;
use crate::service::engine::test_engine::{Workspaces, board, docs, input, kept, settled};
use crate::test_support::{LIST, block, edge, workflow};

/// One engine over workspaces `a` and `b`, at work in `a`; `w` kept in
/// both.
async fn engine_of_two(w: &Workflow) -> Arc<Engine> {
    let (a, b) = (docs(), docs());
    kept(&a, w).await;
    kept(&b, w).await;
    let spaces = Workspaces {
        docs: HashMap::from([("a".to_string(), a), ("b".to_string(), b)]),
        board: board(3),
    };
    Engine::of_spaces(Arc::new(spaces), "a").unwrap()
}

async fn started(engine: &Arc<Engine>) -> Kept<Run> {
    engine
        .start(
            "too-many-tasks",
            &Start::Manual,
            Fired::default(),
            &Input::Map(input(1)),
            None,
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn runs_of_two_workspaces_go_at_once_each_kept_in_its_own() {
    let in_a = engine_of_two(&workflow()).await;
    let in_b = in_a.at("b");
    assert_eq!((in_a.workspace(), in_b.workspace()), ("a", "b"));
    let (run_a, run_b) = tokio::join!(started(&in_a), started(&in_b));
    // Both are driven by the one engine at the same time.
    let (ended_a, ended_b) = tokio::join!(
        settled(&in_a, &run_a.id, false),
        settled(&in_b, &run_b.id, false)
    );
    for ended in [&ended_a, &ended_b] {
        assert_eq!(
            ended.data.status,
            i32::from(RunStatus::Succeeded),
            "{:?}",
            ended.data.failure
        );
    }
    // Each workspace has one run, its own: neither was written in the
    // other's documents.
    let of = |runs: Vec<Kept<Run>>| runs.into_iter().map(|r| r.id).collect::<Vec<_>>();
    assert_eq!(
        of(in_a.runs("too-many-tasks", 0).await.unwrap()),
        std::slice::from_ref(&run_a.id)
    );
    assert_eq!(
        of(in_b.runs("too-many-tasks", 0).await.unwrap()),
        std::slice::from_ref(&run_b.id)
    );
}

#[tokio::test]
async fn an_event_starts_runs_only_in_the_workspace_it_is_of() {
    let mut w = workflow();
    w.input = None;
    let g = w.graph.as_mut().unwrap();
    g.blocks[0] = block(
        "start",
        block::Kind::Trigger(post_cortex_proto::workflow::Trigger {
            kind: Some(trigger::Kind::Event(
                post_cortex_proto::workflow::EventTrigger {
                    name: "board.task_moved".into(),
                    filter: String::new(),
                },
            )),
        }),
    );
    g.blocks[1] = block(
        "list",
        block::Kind::Tool(post_cortex_proto::workflow::ToolBlock {
            method: LIST.into(),
            request: None,
            fixture: None,
        }),
    );
    g.blocks.truncate(2);
    g.edges = vec![edge("start", "list", "")];
    let in_a = engine_of_two(&w).await;
    let in_b = in_a.at("b");
    in_a.listen().await.unwrap();
    in_b.listen().await.unwrap();
    let moved = |workspace: &str| BusEvent {
        name: "board.task_moved".into(),
        module: "board".into(),
        author: "ana".into(),
        workspace_id: workspace.into(),
        ..Default::default()
    };
    // Heard by the engine wherever it is at work: started in `b` only.
    in_a.heard(&moved("b"));
    let mut runs_b = Vec::new();
    for _ in 0..200 {
        runs_b = in_b.runs("too-many-tasks", 0).await.unwrap();
        if !runs_b.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(runs_b.len(), 1, "the event's own workspace");
    settled(&in_b, &runs_b[0].id, false).await;
    assert!(
        in_a.runs("too-many-tasks", 0).await.unwrap().is_empty(),
        "not the other workspace"
    );
    // An event of no workspace is heard in each.
    in_a.heard(&moved(""));
    let mut runs_a = Vec::new();
    for _ in 0..200 {
        runs_a = in_a.runs("too-many-tasks", 0).await.unwrap();
        runs_b = in_b.runs("too-many-tasks", 0).await.unwrap();
        if runs_a.len() == 1 && runs_b.len() == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!((runs_a.len(), runs_b.len()), (1, 2));
}
