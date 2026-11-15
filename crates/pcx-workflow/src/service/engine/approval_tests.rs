// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

// An `approval` in a run (docs/modules/workflow.md, the engine's reach):
// it adds the run's task and waits for its decision, goes on when it is
// approved (heard, or read again after a restart), and withdraws it when
// the run ends first.

use pcx_expr::Value as Expr;
use post_cortex_proto::board::{DecideRequest, Risk};
use post_cortex_proto::team::Stakes;
use post_cortex_proto::workflow::{Approval, block};

use super::*;
use crate::service::engine::test_engine::{
    Board, approved, board, docs, engine, input, kept, settled,
};
use crate::test_support::{ADD, DECIDE, LIST, WITHDRAW, block, catalog, edge, workflow};

/// start -> list -> add, an approval; its output who approved.
fn approving() -> Workflow {
    approving_by(false)
}

/// [`approving`], its approval only a person's when `person`.
fn approving_by(person: bool) -> Workflow {
    let mut w = workflow();
    let g = w.graph.as_mut().unwrap();
    g.blocks.truncate(2);
    g.blocks.push(block(
        "add",
        block::Kind::Approval(Approval {
            question: "Add a task for {{ size(steps.list.tasks) }}?".into(),
            risk: Risk::Code.into(),
            task: String::new(),
            person,
        }),
    ));
    g.edges = vec![edge("start", "list", ""), edge("list", "add", "")];
    w.output = "steps.add.approved_by".into();
    w
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
async fn an_approval_adds_the_runs_task_waits_and_goes_on_when_it_is_approved() {
    let docs = docs();
    kept(&docs, &approving()).await;
    let board = board(3);
    let engine = engine(&docs, board.clone());
    let run = started(&engine).await;
    let waiting = settled(&engine, &run.id, true).await;
    assert_eq!(waiting.data.task, "task-1");
    assert_eq!(board.paths(), [LIST, ADD, DECIDE]);
    engine.heard(&approved("ana"));
    let ended = settled(&engine, &run.id, false).await;
    assert_eq!(
        ended.data.status,
        i32::from(RunStatus::Succeeded),
        "{:?}",
        ended.data.failure
    );
    let output =
        domain::read_value(ended.data.output.as_ref().unwrap(), catalog().messages()).unwrap();
    assert_eq!(output, Expr::String("ana".into()));
    assert!(
        !board.paths().contains(&WITHDRAW.to_string()),
        "an approved decision is not withdrawn"
    );
}

#[tokio::test]
async fn an_approval_only_a_person_approves_asks_its_decision_so() {
    let stakes_of = |board: &Board| -> Option<Stakes> {
        board.decided.lock()[0]
            .transcode_to::<DecideRequest>()
            .unwrap()
            .stakes
    };
    for person in [false, true] {
        let docs = docs();
        kept(&docs, &approving_by(person)).await;
        let board = board(3);
        let engine = engine(&docs, board.clone());
        let run = started(&engine).await;
        settled(&engine, &run.id, true).await;
        let stakes = stakes_of(&board);
        assert_eq!(
            stakes.and_then(|s| s.person),
            person.then_some(true),
            "person {person}"
        );
    }
}

#[tokio::test]
async fn a_run_that_ends_while_its_approval_waits_withdraws_the_decision() {
    let docs = docs();
    kept(&docs, &approving()).await;
    let board = board(3);
    let engine = engine(&docs, board.clone());
    let run = started(&engine).await;
    settled(&engine, &run.id, true).await;
    assert_eq!(board.paths(), [LIST, ADD, DECIDE]);
    engine.cancel(&run.id).await.unwrap();
    let ended = settled(&engine, &run.id, false).await;
    assert_eq!(ended.data.status, i32::from(RunStatus::Cancelled));
    assert_eq!(
        board.paths(),
        [LIST, ADD, DECIDE, WITHDRAW],
        "no one is asked what the run no longer waits for"
    );
    let steps = engine.ports.runs.steps(&run.id).await.unwrap();
    assert!(
        steps
            .iter()
            .all(|s| s.data.status != i32::from(StepStatus::Waiting)),
        "an ended run leaves no step waiting"
    );
}

#[tokio::test]
async fn a_run_that_ended_with_its_approval_waiting_has_it_withdrawn_at_start() {
    let docs = docs();
    kept(&docs, &approving()).await;
    let board = board(3);
    let first = engine(&docs, board.clone());
    let run = started(&first).await;
    settled(&first, &run.id, true).await;
    first.stop();
    // As a run ended before its approvals were withdrawn: failed, its
    // approval's step still waiting.
    let waiting = first.latest(&run.id).await.unwrap();
    let mut failed = waiting.data.clone();
    failed.status = RunStatus::Failed.into();
    first
        .ports
        .runs
        .change(&run.id, waiting.version, &failed)
        .await
        .unwrap();
    let second = engine(&docs, board.clone());
    second.resume().await.unwrap();
    assert_eq!(board.paths(), [LIST, ADD, DECIDE, WITHDRAW]);
    let steps = second.ports.runs.steps(&run.id).await.unwrap();
    assert!(
        steps
            .iter()
            .all(|s| s.data.status != i32::from(StepStatus::Waiting))
    );
    // Settled once: the next start asks the board nothing.
    let third = engine(&docs, board.clone());
    third.resume().await.unwrap();
    assert_eq!(board.paths(), [LIST, ADD, DECIDE, WITHDRAW]);
}

#[tokio::test]
async fn a_run_waiting_on_an_approval_made_while_the_module_was_down_goes_on() {
    let docs = docs();
    kept(&docs, &approving()).await;
    let board = board(3);
    let first = engine(&docs, board.clone());
    let run = started(&first).await;
    settled(&first, &run.id, true).await;
    first.stop();
    *board.approved.lock() = Some("ana".into());
    let second = engine(&docs, board.clone());
    second.resume().await.unwrap();
    let ended = settled(&second, &run.id, false).await;
    let output =
        domain::read_value(ended.data.output.as_ref().unwrap(), catalog().messages()).unwrap();
    assert_eq!(output, Expr::String("ana".into()));
}
