// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

use crate::domain::board;

fn kanban() -> Board {
    board(
        "w24",
        vec!["todo".into(), "doing".into(), "done".into()],
        vec![],
        None,
        "s".into(),
    )
    .unwrap()
}

fn task(column: &str, parent: Option<&str>, depends_on: &[&str], rank: &str) -> Task {
    Task {
        title: "t".into(),
        board: "b".into(),
        column: column.into(),
        parent: parent.map(String::from),
        depends_on: depends_on.iter().map(|d| (*d).to_string()).collect(),
        rank: rank.into(),
        ..Task::default()
    }
}

#[test]
fn a_plan_is_one_level_deep() {
    let plan_task = task("todo", None, &[], "V");
    let child = task("todo", Some("p"), &[], "k");
    let lone = task("todo", None, &[], "s");
    let tasks = [("p", &plan_task), ("c", &child), ("l", &lone)];
    assert_eq!(plan(Some("l"), "p", &tasks), Ok(()));
    assert_eq!(plan(None, "p", &tasks), Ok(()));
    assert_eq!(plan(Some("p"), "p", &tasks), Err(Broken::SelfLink));
    assert_eq!(
        plan(Some("l"), "c", &tasks),
        Err(Broken::Nested { plan: "c".into() })
    );
    assert_eq!(
        plan(Some("p"), "l", &tasks),
        Err(Broken::IsPlan { task: "p".into() })
    );
    assert_eq!(
        plan(Some("l"), "x", &tasks),
        Err(Broken::NotOnBoard { task: "x".into() })
    );
}

#[test]
fn a_dependency_that_makes_a_cycle_names_the_chain() {
    let a = task("todo", None, &["b"], "");
    let b = task("todo", None, &["c"], "");
    let c = task("todo", None, &[], "");
    let tasks = [("a", &a), ("b", &b), ("c", &c)];
    assert_eq!(
        dependencies(Some("c"), &["a".into()], &tasks),
        Err(Broken::Cycle {
            chain: vec!["c".into(), "a".into(), "b".into(), "c".into()]
        })
    );
    assert_eq!(
        dependencies(Some("c"), &["c".into()], &tasks),
        Err(Broken::SelfLink)
    );
    assert_eq!(
        dependencies(Some("a"), &[" b".into(), "c".into(), "b".into()], &tasks),
        Ok(vec!["b".into(), "c".into()])
    );
    assert_eq!(
        dependencies(None, &["z".into()], &tasks),
        Err(Broken::NotOnBoard { task: "z".into() })
    );
    assert!(
        Broken::Cycle {
            chain: vec!["c".into(), "a".into(), "c".into()]
        }
        .to_string()
        .ends_with("c waits on a waits on c")
    );
}

#[test]
fn ready_progress_and_what_waits_are_worked_out() {
    let board = kanban();
    let plan_task = task("doing", None, &[], "");
    let done = task("done", Some("p"), &[], "");
    let open = task("todo", Some("p"), &["d"], "");
    let blocked = task("todo", None, &["d", "o"], "");
    let tasks = [
        ("p", &plan_task),
        ("d", &done),
        ("o", &open),
        ("x", &blocked),
    ];
    let of_plan = standing(&board, "p", &plan_task, &tasks);
    assert_eq!(of_plan.progress, Some(Progress { done: 1, total: 2 }));
    assert!(of_plan.ready, "it waits on nothing");
    let of_open = standing(&board, "o", &open, &tasks);
    assert!(of_open.ready, "what it waits on is done");
    assert_eq!(of_open.waited_on_by, ["x"]);
    assert_eq!(of_open.progress, None);
    assert!(!standing(&board, "x", &blocked, &tasks).ready);
    assert_eq!(
        standing(&board, "d", &done, &tasks).waited_on_by,
        ["o", "x"]
    );
}

#[test]
fn a_move_ranks_the_task_between_its_neighbours() {
    let first = task("todo", None, &[], "F");
    let second = task("todo", None, &[], "V");
    let old = task("todo", None, &[], "");
    let moving = task("doing", None, &[], "k");
    let tasks = [("f", &first), ("s", &second), ("o", &old), ("m", &moving)];
    let between = place("todo", Some("m"), Some("s"), &tasks).unwrap();
    assert!("F" < between.as_str() && between.as_str() < "V");
    let start = place("todo", Some("m"), Some("f"), &tasks).unwrap();
    assert!(start.as_str() < "F");
    // At the end, or before a task without a rank: after the ranked ones.
    let end = place("todo", Some("m"), None, &tasks).unwrap();
    assert!(end.as_str() > "V");
    assert_eq!(place("todo", Some("m"), Some("o"), &tasks).unwrap(), end);
    assert_eq!(place("todo", None, None, &tasks).unwrap(), end);
    assert_eq!(
        place("todo", Some("m"), Some("m"), &tasks),
        Err(Broken::SelfLink)
    );
    assert_eq!(
        place("doing", Some("f"), Some("s"), &tasks),
        Err(Broken::NotInColumn {
            task: "s".into(),
            column: "doing".into()
        })
    );
    let mut column: Vec<(&str, &Task)> = vec![("o", &old), ("s", &second), ("f", &first)];
    order(&mut column);
    assert_eq!(
        column.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        ["f", "s", "o"]
    );
}

#[test]
fn a_canceled_task_is_waited_on_no_longer_and_counts_in_no_plan() {
    let board = board(
        "w26",
        vec!["todo".into(), "done".into(), "canceled".into()],
        vec![
            Stage::Open.into(),
            Stage::Done.into(),
            Stage::Canceled.into(),
        ],
        None,
        "s".into(),
    )
    .unwrap();
    let plan_task = task("todo", None, &[], "");
    let done = task("done", Some("p"), &[], "");
    let dropped = task("canceled", Some("p"), &[], "");
    let open = task("todo", Some("p"), &["c"], "");
    let tasks = [
        ("p", &plan_task),
        ("d", &done),
        ("c", &dropped),
        ("o", &open),
    ];
    let of_plan = standing(&board, "p", &plan_task, &tasks);
    assert_eq!(of_plan.progress, Some(Progress { done: 1, total: 2 }));
    assert!(standing(&board, "o", &open, &tasks).ready);
}
