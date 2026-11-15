// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use std::collections::BTreeMap;

use chrono::TimeDelta;
use pcx_module_api::Effect;

use post_cortex_proto::board::{Task, TaskList, TaskRecord};
use post_cortex_proto::workflow::{Fired, ForEach, Graph, Retry, StepStatus, block};
use prost_reflect::DynamicMessage;

use super::*;
use crate::domain::check;
use crate::domain::value::write;
use crate::test_support::{block, catalog, edge, set, workflow};

/// A run of `workflow` driven in memory: each act done at once, a tool
/// answered by `answer` (its block's id and attempt).
struct Driven {
    journal: Journal,
    calls: Vec<(String, String, u32)>,
    /// The most acts started at once.
    widest: usize,
}

fn tasks(n: usize) -> DynamicMessage {
    let list = TaskList {
        tasks: (0..n)
            .map(|i| TaskRecord {
                id: format!("t{i}"),
                task: Some(Task {
                    title: format!("Task {i}"),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    };
    as_message(&list, catalog().messages()).unwrap()
}

fn step(act: &Act, status: StepStatus) -> Step {
    Step {
        at: act.at.clone(),
        block: act.block.clone(),
        attempt: act.attempt,
        status: status.into(),
        ended_at: Some(prost_types::Timestamp {
            seconds: 100,
            nanos: 0,
        }),
        ..Default::default()
    }
}

fn drive(
    w: &Workflow,
    input: Values,
    answer: impl Fn(&str, u32, &DynamicMessage) -> Result<DynamicMessage, Failure>,
) -> (Next, Driven) {
    let catalog = catalog();
    let checked = check(w, &catalog, &BTreeMap::new()).unwrap();
    let fired = Expr::Message(as_message(&Fired::default(), catalog.messages()).unwrap());
    let mut driven = Driven {
        journal: Journal::new(
            [Step {
                block: "start".into(),
                attempt: 1,
                status: StepStatus::Succeeded.into(),
                ..Default::default()
            }],
            catalog.messages(),
        ),
        calls: Vec::new(),
        widest: 0,
    };
    for _ in 0..200 {
        let next = Plan {
            workflow: w,
            checked: &checked,
            catalog: &catalog,
            input: &input,
            trigger: ("start", &fired),
            journal: &driven.journal,
            now: DateTime::UNIX_EPOCH,
        }
        .next();
        let Next::Act(acts) = next else {
            return (next, driven);
        };
        assert!(!acts.is_empty(), "a busy run with nothing to do");
        driven.widest = driven.widest.max(acts.len());
        for act in acts {
            let written = match &act.work {
                Work::Tool { request, .. } => {
                    driven
                        .calls
                        .push((act.at.clone(), act.block.clone(), act.attempt));
                    match answer(&act.block, act.attempt, request) {
                        Ok(response) => Step {
                            output: Some(write(&Expr::Message(response)).unwrap()),
                            port: "next".into(),
                            ..step(&act, StepStatus::Succeeded)
                        },
                        Err(failure) => Step {
                            failure: Some(failure),
                            ..step(&act, StepStatus::Failed)
                        },
                    }
                }
                Work::Done { output, port } => Step {
                    output: Some(write(output).unwrap()),
                    port: port.clone(),
                    ..step(&act, StepStatus::Succeeded)
                },
                Work::Open { input } => Step {
                    input: Some(write(input).unwrap()),
                    ..step(&act, StepStatus::Running)
                },
                Work::Close { output } => Step {
                    output: Some(write(output).unwrap()),
                    port: "next".into(),
                    ..step(&act, StepStatus::Succeeded)
                },
                Work::Fail { failure } => Step {
                    failure: Some(failure.clone()),
                    ..step(&act, StepStatus::Failed)
                },
                other => panic!("not driven here: {other:?}"),
            };
            let written = match driven.journal.last(&act.at, &act.block) {
                // An open step keeps its input when it closes.
                Some(open) if open.step.attempt == written.attempt => Step {
                    input: open.input().map(|i| write(&i.unwrap()).unwrap()),
                    ..written
                },
                _ => written,
            };
            driven.journal.record(written);
        }
    }
    panic!("the run does not end");
}

fn input(board: &str, limit: i64) -> Values {
    Values::from([
        ("input.board".to_string(), Expr::String(board.into())),
        ("input.limit".to_string(), Expr::Int(limit)),
    ])
}

fn board_of(request: &DynamicMessage) -> String {
    request
        .get_field_by_name("board")
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

#[test]
fn a_run_follows_its_branch_and_ends_with_its_output() {
    let (next, driven) = drive(&workflow(), input("b1", 1), |block, _, request| {
        assert_eq!(board_of(request), "b1");
        match block {
            "list" => Ok(tasks(3)),
            "add" => {
                let title = request.get_field_by_name("title").unwrap();
                assert_eq!(title.as_str(), Some("Too many: 3 on "));
                let labels = request.get_field_by_name("labels").unwrap();
                assert_eq!(labels.as_list().unwrap().len(), 2);
                let record = TaskRecord::default();
                Ok(as_message(&record, catalog().messages()).unwrap())
            }
            other => panic!("{other}"),
        }
    });
    let Next::Succeeded(output) = next else {
        panic!("{next:?}")
    };
    assert_eq!(output, Expr::Int(3));
    let blocks: Vec<_> = driven.calls.iter().map(|(_, b, _)| b.as_str()).collect();
    assert_eq!(blocks, ["list", "add"]);
    assert_eq!(
        driven.journal.last("", "too_many").unwrap().step.port,
        "then"
    );
}

#[test]
fn a_read_that_fails_is_tried_again_after_its_backoff_then_fails_the_run() {
    let mut w = workflow();
    w.graph.as_mut().unwrap().blocks[1].retry = Some(Retry { attempts: 3 });
    let (next, driven) = drive(&w, input("b1", 9), |_, _, _| {
        Err(failure("internal", "the board broke"))
    });
    let Next::Failed(failed) = next else {
        panic!("{next:?}")
    };
    assert_eq!(failed.message, "the board broke");
    assert_eq!(
        driven.calls.iter().map(|c| c.2).collect::<Vec<_>>(),
        [1, 2, 3]
    );
    assert_eq!(backoff(1), TimeDelta::seconds(1));
    assert_eq!(backoff(3), TimeDelta::seconds(4));
    assert_eq!(backoff(40), TimeDelta::hours(1));
    assert!(!retryable(Some(Effect::Write), "internal"));
    assert!(retryable(Some(Effect::Write), "unavailable"));
    assert!(!retryable(Some(Effect::Read), "invalid_argument"));
}

#[test]
fn an_error_edge_takes_the_run_on_with_the_failure() {
    let mut w = workflow();
    let g = w.graph.as_mut().unwrap();
    g.blocks.push(set("failed", "why", "steps.list.message"));
    g.edges.push(edge("list", "failed", "error"));
    w.output = String::new();
    let (next, driven) = drive(&w, input("b1", 9), |_, _, _| {
        Err(failure("not_found", "no board b1"))
    });
    assert!(matches!(next, Next::Succeeded(Expr::Null)), "{next:?}");
    let why = driven.journal.last("", "failed").unwrap().output().unwrap();
    assert_eq!(
        why,
        Expr::map([("why".to_string(), Expr::string("no board b1"))])
    );
}

#[test]
fn a_for_each_runs_its_bodies_at_most_its_concurrency_at_once_and_gives_their_outputs() {
    let mut w = workflow();
    let g = w.graph.as_mut().unwrap();
    g.blocks[4] = block(
        "each",
        block::Kind::ForEach(ForEach {
            items: "steps.list.tasks".into(),
            concurrency: 2,
            body: Some(Graph {
                blocks: vec![set(
                    "pick",
                    "title",
                    "item.task.title + '/' + string(index)",
                )],
                edges: vec![],
                output: "steps.pick.title".into(),
            }),
        }),
    );
    g.edges[3].to = "each".into();
    w.output = String::new();
    // limit 5 with 3 tasks: the else branch.
    let (next, driven) = drive(&w, input("b1", 5), |_, _, _| Ok(tasks(3)));
    assert!(matches!(next, Next::Succeeded(Expr::Null)), "{next:?}");
    let each = driven.journal.last("", "each").unwrap().output().unwrap();
    assert_eq!(
        each,
        Expr::list(["Task 0/0", "Task 1/1", "Task 2/2"].map(Expr::string))
    );
    assert!(driven.journal.last("each#2", "pick").is_some());
    assert_eq!(driven.widest, 2, "two bodies at once");
}
