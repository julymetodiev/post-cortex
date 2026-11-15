// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The board's events: a new task, a task that changes column (not one
//! that stays, nor one a key finds again) and a decision, each after its
//! write.

use super::*;

#[tokio::test]
async fn a_new_task_a_move_and_a_decision_are_events() {
    let (docs, memory) = (documents(), Memory::default());
    let board = Service::new(&docs, &memory, &Git, &memory, Some("ana"), None);
    let created = board
        .create_board("Sprint", columns(), vec![], None)
        .await
        .unwrap();
    let add = || AddTaskRequest {
        board: created.document.id.clone(),
        title: "Ship".into(),
        key: Some("ship".into()),
        ..Default::default()
    };
    let task = board.add_task(add()).await.unwrap().task.task;
    // Found again by its key: no new task, no event.
    assert!(board.add_task(add()).await.unwrap().existing);
    let id = task.document.id.clone();
    let stayed = board
        .move_task(to(&id, task.document.version, "todo"))
        .await
        .unwrap()
        .task;
    board
        .move_task(to(&id, stayed.document.version, "doing"))
        .await
        .unwrap();
    let decided = board
        .decide(&id, "ship it", "ready", Risk::None, Stakes::default())
        .await
        .unwrap();

    let board_id = created.document.id;
    assert_eq!(
        *memory.events.lock(),
        vec![
            BoardEvent::TaskCreated {
                task: id.clone(),
                board: board_id.clone(),
                title: "Ship".into(),
                column: "todo".into(),
            },
            BoardEvent::TaskMoved {
                task: id.clone(),
                board: board_id,
                from: "todo".into(),
                to: "doing".into(),
            },
            BoardEvent::DecisionMade {
                decision: decided.document.id,
                task: id,
                pending: false,
            },
        ]
    );
}
