// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Plans, dependencies, order, the approvals (W24), a task's assignee,
//! canceling (W26) and keys (W27).

use super::*;

use crate::domain::Stage;
use post_cortex_proto::board::{Progress, TaskIds};

async fn add(board: &Service<'_>, on: &str, title: &str, parent: Option<&str>) -> Placed {
    board
        .add_task(AddTaskRequest {
            board: on.to_string(),
            title: title.into(),
            parent: parent.map(String::from),
            ..Default::default()
        })
        .await
        .unwrap()
        .task
}

#[tokio::test]
async fn a_plan_counts_its_tasks_and_a_task_waits_on_others() {
    let (docs, memory) = (documents(), Memory::default());
    let board = Service::new(&docs, &memory, &Git, &memory, Some("ana"), None);
    let on = board
        .create_board("W24", columns(), vec![], None)
        .await
        .unwrap();
    let on = on.document.id.as_str();
    let plan = add(&board, on, "Board v2", None).await;
    let plan_id = plan.task.document.id.clone();
    let proto = add(&board, on, "proto", Some(&plan_id)).await;
    let view = add(&board, on, "view", Some(&plan_id)).await;
    // A plan's task is not a plan, and a plan has no plan.
    let nested = board
        .add_task(AddTaskRequest {
            board: on.into(),
            title: "deeper".into(),
            parent: Some(proto.task.document.id.clone()),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(nested.kind, ModuleErrorKind::InvalidArgument);

    let waits = board
        .update_task(UpdateTaskRequest {
            task: view.task.document.id.clone(),
            version: view.task.document.version,
            depends_on: Some(TaskIds {
                tasks: vec![proto.task.document.id.clone()],
            }),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(!waits.standing.ready);
    let cycle = board
        .update_task(UpdateTaskRequest {
            task: proto.task.document.id.clone(),
            version: proto.task.document.version,
            depends_on: Some(TaskIds {
                tasks: vec![view.task.document.id.clone()],
            }),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(cycle.kind, ModuleErrorKind::InvalidArgument);
    assert!(cycle.message.contains("cycle"), "{cycle}");

    board
        .move_task(to(
            &proto.task.document.id,
            proto.task.document.version,
            "done",
        ))
        .await
        .unwrap();
    let listed = board.tasks(on, None).await.unwrap().tasks;
    let of = |id: &str| {
        listed
            .iter()
            .find(|p| p.task.document.id == id)
            .unwrap()
            .standing
            .clone()
    };
    assert_eq!(
        of(&plan_id).progress,
        Some(post_cortex_proto::board::Progress { done: 1, total: 2 })
    );
    assert!(of(&view.task.document.id).ready);
    assert_eq!(
        of(&proto.task.document.id).waited_on_by,
        [view.task.document.id.as_str()]
    );
    let plans = board.tasks(on, None).await.unwrap().plans;
    assert_eq!(
        plans
            .iter()
            .map(|p| p.task.document.id.as_str())
            .collect::<Vec<_>>(),
        [plan_id.as_str()]
    );
    let thread = board.thread(&view.task.document.id).await.unwrap();
    let ids = |placed: &[Placed]| -> Vec<String> {
        placed.iter().map(|p| p.task.document.id.clone()).collect()
    };
    assert_eq!(
        ids(&thread.related.waits_on),
        std::slice::from_ref(&proto.task.document.id)
    );
    assert_eq!(
        thread.related.plan.map(|p| p.task.document.id),
        Some(plan_id.clone())
    );
    assert_eq!(thread.related.plan_tasks.len(), 2);
    let of_plan = board.thread(&plan_id).await.unwrap();
    assert_eq!(of_plan.related.plan_tasks.len(), 2, "a plan lists its own");
    // In the columns' order: todo first, done last.
    assert_eq!(
        listed.last().unwrap().task.document.id,
        proto.task.document.id
    );
}

#[tokio::test]
async fn a_move_places_the_task_before_another_and_in_another_plan() {
    let (docs, memory) = (documents(), Memory::default());
    let board = Service::new(&docs, &memory, &Git, &memory, Some("ana"), None);
    let on = board
        .create_board("W24", columns(), vec![], None)
        .await
        .unwrap();
    let on = on.document.id.as_str();
    let first = add(&board, on, "first", None).await;
    let second = add(&board, on, "second", None).await;
    let plan = add(&board, on, "plan", None).await;
    let titles = |tasks: Vec<Placed>| -> Vec<String> {
        tasks.into_iter().map(|t| t.task.data.title).collect()
    };
    assert_eq!(
        titles(board.tasks(on, Some("todo")).await.unwrap().tasks),
        ["first", "second", "plan"]
    );
    let moved = board
        .move_task(MoveTaskRequest {
            before: Some(first.task.document.id.clone()),
            parent: Some(plan.task.document.id.clone()),
            ..to(
                &second.task.document.id,
                second.task.document.version,
                "todo",
            )
        })
        .await
        .unwrap();
    assert_eq!(
        moved.task.data.parent.as_deref(),
        Some(plan.task.document.id.as_str())
    );
    assert_eq!(
        titles(board.tasks(on, Some("todo")).await.unwrap().tasks),
        ["second", "first", "plan"]
    );
    let out = board
        .move_task(MoveTaskRequest {
            parent: Some(String::new()),
            ..to(
                &second.task.document.id,
                moved.task.document.version,
                "doing",
            )
        })
        .await
        .unwrap();
    assert_eq!(out.task.data.parent, None);
    let nowhere = board
        .move_task(MoveTaskRequest {
            before: Some(first.task.document.id.clone()),
            ..to(&out.task.document.id, out.task.document.version, "done")
        })
        .await
        .unwrap_err();
    assert_eq!(nowhere.kind, ModuleErrorKind::InvalidArgument);
}

#[tokio::test]
async fn the_approvals_list_what_waits_and_a_note_is_recorded() {
    let (docs, memory) = (documents(), Memory::default());
    let agent = Service::new(&docs, &memory, &Git, &memory, Some("agent"), None);
    let on = agent
        .create_board("Ops", columns(), vec![], None)
        .await
        .unwrap();
    let task = add(&agent, &on.document.id, "Clean up", None).await;
    let id = task.task.document.id.as_str();
    agent
        .decide(id, "keep it", "small", Risk::None, Stakes::default())
        .await
        .unwrap();
    let risky = agent
        .decide(id, "drop it", "old", Risk::Data, Stakes::default())
        .await
        .unwrap();
    assert_eq!(
        risky.data.held_because.as_deref(),
        Some("agent is in no team of the workspace: a person approves")
    );
    agent
        .comment(id, "after the backup?", None, Vec::new())
        .await
        .unwrap();
    let listed = agent.tasks(&on.document.id, None).await.unwrap().tasks;
    assert_eq!(
        (listed[0].comments, listed[0].held_by.as_deref()),
        (1, Some("drop it"))
    );
    let waiting = agent.decisions(None).await.unwrap();
    assert_eq!(
        waiting.len(),
        1,
        "a decision that risks nothing is not listed"
    );
    assert_eq!(
        (
            waiting[0].task_title.as_str(),
            waiting[0].board_name.as_str()
        ),
        ("Clean up", "Ops")
    );

    let person = Service::new(&docs, &memory, &Git, &memory, Some("julius"), None);
    let approved = person
        .approve(
            &risky.document.id,
            risky.document.version,
            Some("after the backup".into()),
        )
        .await
        .unwrap();
    assert_eq!(
        approved.data.approval_note.as_deref(),
        Some("after the backup")
    );
    let (_, record) = memory.decisions.lock().last().cloned().unwrap();
    assert_eq!(
        (
            record.what.as_str(),
            record.why.as_str(),
            record.decided_by.as_str()
        ),
        ("Approved: drop it", "after the backup", "julius")
    );
    let listed = person.decisions(Some(false)).await.unwrap();
    assert_eq!(
        listed[0].decision.data.approved_by.as_deref(),
        Some("julius")
    );
    assert!(person.decisions(Some(true)).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_withdrawn_decision_holds_nothing_and_is_not_listed() {
    let (docs, memory) = (documents(), Memory::default());
    let agent = Service::new(&docs, &memory, &Git, &memory, Some("agent"), None);
    let on = agent
        .create_board("Ops", columns(), vec![], None)
        .await
        .unwrap();
    let task = add(&agent, &on.document.id, "Clean up", None).await;
    let id = task.task.document.id.as_str();
    let risky = agent
        .decide(id, "drop it", "old", Risk::Data, Stakes::default())
        .await
        .unwrap();
    let person = Service::new(&docs, &memory, &Git, &memory, Some("julius"), None);
    let refused = person
        .withdraw(&risky.document.id, "not mine")
        .await
        .err()
        .unwrap();
    assert_eq!(refused.kind, pcx_module_api::ModuleErrorKind::Denied);
    let withdrawn = agent
        .withdraw(&risky.document.id, "its run ended")
        .await
        .unwrap();
    assert_eq!(withdrawn.data.withdrawn.as_deref(), Some("its run ended"));
    assert!(person.decisions(None).await.unwrap().is_empty());
    let listed = agent.tasks(&on.document.id, None).await.unwrap().tasks;
    assert_eq!(listed[0].held_by, None, "it holds its task no more");
    let approve = person
        .approve(&risky.document.id, withdrawn.document.version, None)
        .await
        .err()
        .unwrap();
    assert_eq!(
        approve.kind,
        pcx_module_api::ModuleErrorKind::FailedPrecondition
    );
}

#[tokio::test]
async fn a_declined_decision_holds_nothing_is_not_listed_and_is_said_on_the_bus() {
    let (docs, memory) = (documents(), Memory::default());
    let agent = Service::new(&docs, &memory, &Git, &memory, Some("agent"), None);
    let on = agent
        .create_board("Ops", columns(), vec![], None)
        .await
        .unwrap();
    let task = add(&agent, &on.document.id, "Restart", None).await;
    let risky = agent
        .decide(
            task.task.document.id.as_str(),
            "restart it",
            "it loops",
            Risk::Operations,
            Stakes::default(),
        )
        .await
        .unwrap();
    let person = Service::new(&docs, &memory, &Git, &memory, Some("julius"), None);
    let declined = person
        .decline(&risky.document.id, risky.document.version, None)
        .await
        .unwrap();
    assert_eq!(declined.data.declined_by.as_deref(), Some("julius"));
    assert!(person.decisions(None).await.unwrap().is_empty());
    let listed = agent.tasks(&on.document.id, None).await.unwrap().tasks;
    assert_eq!(listed[0].held_by, None, "it holds its task no more");
    assert!(
        memory
            .events
            .lock()
            .contains(&BoardEvent::DecisionDeclined {
                decision: risky.document.id.clone(),
                task: task.task.document.id.clone(),
                declined_by: "julius".into(),
            })
    );
}

#[tokio::test]
async fn an_empty_assignee_leaves_the_task_to_no_one() {
    let (docs, memory) = (documents(), Memory::default());
    let board = Service::new(&docs, &memory, &Git, &memory, Some("ana"), None);
    let on = board
        .create_board("Ops", columns(), vec![], None)
        .await
        .unwrap();
    let task = add(&board, &on.document.id, "Clean up", None).await.task;
    let given = board
        .update_task(UpdateTaskRequest {
            task: task.document.id.clone(),
            version: task.document.version,
            assignee: Some("dev".into()),
            ..Default::default()
        })
        .await
        .unwrap()
        .task;
    assert_eq!(given.data.assignee.as_deref(), Some("dev"));
    let freed = board
        .update_task(UpdateTaskRequest {
            task: task.document.id.clone(),
            version: given.document.version,
            assignee: Some(String::new()),
            ..Default::default()
        })
        .await
        .unwrap()
        .task;
    assert_eq!(freed.data.assignee, None);
}

#[tokio::test]
async fn a_canceled_task_is_kept_with_its_note_and_canceling_needs_close() {
    let (docs, memory) = (documents(), Memory::default());
    team(&docs).await;
    let ana = Service::new(&docs, &memory, &Git, &memory, Some("ana"), None);
    let stages = vec![
        Stage::Open.into(),
        Stage::Done.into(),
        Stage::Canceled.into(),
    ];
    let columns = vec!["todo".into(), "done".into(), "canceled".into()];
    let on = ana
        .create_board("Ops", columns, stages, None)
        .await
        .unwrap();
    let plan = add(&ana, &on.document.id, "Launch", None).await;
    let task = add(
        &ana,
        &on.document.id,
        "Old idea",
        Some(&plan.task.document.id),
    )
    .await;
    let cancel = |version| MoveTaskRequest {
        note: Some("Superseded by the new launch".into()),
        ..to(&task.task.document.id, version, "canceled")
    };

    let bot = Service::new(&docs, &memory, &Git, &memory, Some("bot"), None);
    let refused = bot
        .move_task(cancel(task.task.document.version))
        .await
        .unwrap_err();
    assert_eq!(refused.kind, ModuleErrorKind::Denied);
    let blank = MoveTaskRequest {
        note: Some(" ".into()),
        ..cancel(task.task.document.version)
    };
    let refused = ana.move_task(blank).await.unwrap_err();
    assert_eq!(refused.kind, ModuleErrorKind::InvalidArgument);

    let canceled = ana
        .move_task(cancel(task.task.document.version))
        .await
        .unwrap();
    assert_eq!(canceled.task.data.column, "canceled");
    let thread = ana.thread(&task.task.document.id).await.unwrap();
    assert_eq!(thread.comments.len(), 1, "a refused move writes no note");
    assert_eq!(
        thread.comments[0].document.text,
        "Superseded by the new launch"
    );
    let of_plan = ana.thread(&plan.task.document.id).await.unwrap().task;
    assert_eq!(
        of_plan.standing.progress,
        Some(Progress { done: 0, total: 0 }),
        "a canceled task counts in no plan"
    );

    let reopened = ana
        .move_task(to(
            &task.task.document.id,
            canceled.task.document.version,
            "todo",
        ))
        .await
        .unwrap();
    assert_eq!(reopened.task.data.column, "todo");
}

#[tokio::test]
async fn a_key_the_board_knows_answers_its_task_unchanged() {
    let (docs, memory) = (documents(), Memory::default());
    let board = Service::new(&docs, &memory, &Git, &memory, Some("ana"), None);
    let on = board
        .create_board("Ops", columns(), vec![], None)
        .await
        .unwrap();
    let keyed = |title: &str, key: &str| AddTaskRequest {
        board: on.document.id.clone(),
        title: title.into(),
        key: Some(key.into()),
        ..Default::default()
    };
    let first = board
        .add_task(keyed("Disk full", " group-1 "))
        .await
        .unwrap();
    assert!(!first.existing);
    assert_eq!(first.task.task.data.key, "group-1");
    let again = board
        .add_task(keyed("Disk full again", "group-1"))
        .await
        .unwrap();
    assert!(again.existing);
    assert_eq!(again.task.task.document.id, first.task.task.document.id);
    assert_eq!(
        again.task.task.data.title, "Disk full",
        "answered unchanged"
    );
    let other = board.add_task(keyed("Other", "group-2")).await.unwrap();
    assert!(!other.existing);
    let unkeyed = board.add_task(keyed("Blank key", "  ")).await.unwrap();
    assert_eq!(unkeyed.task.task.data.key, "");
    let long = board
        .add_task(keyed("Long", &"k".repeat(201)))
        .await
        .unwrap_err();
    assert_eq!(long.kind, ModuleErrorKind::InvalidArgument);
    assert_eq!(
        board
            .tasks(&on.document.id, None)
            .await
            .unwrap()
            .tasks
            .len(),
        3
    );
}
