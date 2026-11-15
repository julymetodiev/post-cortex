// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use pcx_expr::Value as Expr;
use post_cortex_proto::workflow::block;

use super::*;
use crate::adapters::SystemClock;
use crate::service::engine::test_engine::{board, docs, engine, input, kept, scope_of, settled};
use crate::test_support::{ADD, LIST, WITHDRAW, block, catalog, edge, workflow};

#[tokio::test]
async fn a_run_calls_its_tools_writes_its_journal_and_ends_with_its_output() {
    let docs = docs();
    kept(&docs, &workflow()).await;
    let board = board(3);
    let engine = engine(&docs, board.clone());
    let run = engine
        .start(
            "too-many-tasks",
            &Start::Manual,
            Fired::default(),
            &Input::Map(input(1)),
            None,
        )
        .await
        .unwrap();
    let ended = settled(&engine, &run.id, false).await;
    assert_eq!(
        ended.data.status,
        i32::from(RunStatus::Succeeded),
        "{:?}",
        ended.data.failure
    );
    let output =
        domain::read_value(ended.data.output.as_ref().unwrap(), catalog().messages()).unwrap();
    assert_eq!(output, Expr::Int(3));
    assert_eq!(board.paths(), [LIST, ADD]);
    let steps = engine.ports.runs.steps(&run.id).await.unwrap();
    let mut blocks: Vec<_> = steps.iter().map(|s| s.data.block.clone()).collect();
    blocks.sort();
    assert_eq!(blocks, ["add", "list", "start", "too_many"]);
    assert!(steps.iter().all(|s| s.data.ended_at.is_some()));
}

#[tokio::test]
async fn a_bad_input_is_refused_before_a_run_starts() {
    let docs = docs();
    kept(&docs, &workflow()).await;
    let engine = engine(&docs, board(0));
    let mut wrong = input(1);
    wrong.entries.remove("board");
    let refused = engine
        .start(
            "too-many-tasks",
            &Start::Manual,
            Fired::default(),
            &Input::Map(wrong),
            None,
        )
        .await
        .err()
        .unwrap();
    assert_eq!(refused.kind, ModuleErrorKind::InvalidArgument);
    let call = engine
        .start(
            "too-many-tasks",
            &Start::Call,
            Fired::default(),
            &Input::Map(input(1)),
            None,
        )
        .await
        .err()
        .unwrap();
    assert_eq!(
        call.kind,
        ModuleErrorKind::FailedPrecondition,
        "it has no call trigger"
    );
}

#[tokio::test]
async fn a_run_goes_on_after_a_restart_from_its_journal() {
    let docs = docs();
    kept(&docs, &workflow()).await;
    let first = engine(&docs, board(3));
    // The run as a crash left it: its trigger's step written, nothing else.
    let run = first
        .ports
        .runs
        .create(&Run {
            workflow: "too-many-tasks".into(),
            workflow_id: first
                .ports
                .workflows
                .named("too-many-tasks")
                .await
                .unwrap()
                .unwrap()
                .id,
            version: 1,
            trigger: Some(Fired {
                block: "start".into(),
                ..Default::default()
            }),
            input: Some(input(5)),
            status: RunStatus::Running.into(),
            started_at: Some(now_ts(&SystemClock)),
            ..Default::default()
        })
        .await
        .unwrap();
    first
        .ports
        .runs
        .put_step(&Step {
            run: run.id.clone(),
            block: "start".into(),
            attempt: 1,
            status: StepStatus::Succeeded.into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let board = board(3);
    let second = engine(&docs, board.clone());
    assert_eq!(second.resume().await.unwrap(), 1);
    let ended = settled(&second, &run.id, false).await;
    assert_eq!(
        ended.data.status,
        i32::from(RunStatus::Succeeded),
        "{:?}",
        ended.data.failure
    );
    // limit 5, 3 tasks: the else branch, no task added.
    let calls: Vec<_> = board.calls.lock().iter().map(|(p, _)| p.clone()).collect();
    assert_eq!(calls, [LIST]);
}

#[tokio::test]
async fn an_event_whose_filter_holds_starts_its_workflow() {
    let docs = docs();
    let mut w = workflow();
    w.input = None;
    let g = w.graph.as_mut().unwrap();
    g.blocks[0] = block(
        "start",
        block::Kind::Trigger(post_cortex_proto::workflow::Trigger {
            kind: Some(trigger::Kind::Event(
                post_cortex_proto::workflow::EventTrigger {
                    name: "board.task_moved".into(),
                    filter: "event.module == \"board\"".into(),
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
    kept(&docs, &w).await;
    let board = board(1);
    let engine = engine(&docs, board.clone());
    engine.listen().await.unwrap();
    let moved = |module: &str, author: &str| BusEvent {
        name: "board.task_moved".into(),
        module: module.into(),
        author: author.into(),
        ..Default::default()
    };
    engine.heard(&moved("mail", "ana"));
    engine.heard(&moved("board", "workflow:too-many-tasks"));
    engine.heard(&moved("board", "ana"));
    let mut runs = Vec::new();
    for _ in 0..200 {
        runs = engine.runs("too-many-tasks", 0).await.unwrap();
        if !runs.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(runs.len(), 1, "only the board's, not one it fed itself");
    let ended = settled(&engine, &runs[0].id, false).await;
    assert_eq!(ended.data.trigger.unwrap().event.unwrap().author, "ana");
}

/// Discord's `Status` as the host lists it: a tool of a module that is not
/// the board, whose descriptors hold its events.
fn discord_status() -> (String, crate::domain::Tool) {
    let pool =
        prost_reflect::DescriptorPool::decode(post_cortex_proto::DISCORD_DESCRIPTOR_SET).unwrap();
    let service = pool.get_service_by_name("pcx.discord.v1.Discord").unwrap();
    let method = service.methods().find(|m| m.name() == "Status").unwrap();
    (
        "/pcx.discord.v1.Discord/Status".to_string(),
        crate::domain::Tool {
            name: "discord_status".into(),
            description: String::new(),
            request_schema: serde_json::Map::new(),
            response_schema: serde_json::Map::new(),
            module: "discord".into(),
            effect: pcx_module_api::Effect::Read,
            request: method.input(),
            response: method.output(),
        },
    )
}

#[tokio::test]
async fn a_filter_reads_the_payload_of_an_event_of_a_module_the_workflow_reaches() {
    use prost::{Message, Name};
    let docs = docs();
    let mut w = workflow();
    w.input = None;
    let g = w.graph.as_mut().unwrap();
    g.blocks[0] = block(
        "start",
        block::Kind::Trigger(post_cortex_proto::workflow::Trigger {
            kind: Some(trigger::Kind::Event(
                post_cortex_proto::workflow::EventTrigger {
                    name: "discord.post_created".into(),
                    filter: "event.payload.channel == \"help\"".into(),
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
    kept(&docs, &w).await;
    let board = board(1);
    board.others.lock().push(discord_status());
    let engine = engine(&docs, board.clone());
    engine.listen().await.unwrap();
    let post = |channel: &str| {
        let created = post_cortex_proto::discord::PostCreated {
            channel: channel.into(),
            title: channel.into(),
            ..Default::default()
        };
        BusEvent {
            name: "discord.post_created".into(),
            module: "discord".into(),
            author: "discord".into(),
            payload: Some(pbjson_types::Any {
                type_url: post_cortex_proto::discord::PostCreated::type_url(),
                value: created.encode_to_vec().into(),
            }),
            ..Default::default()
        }
    };
    engine.heard(&post("feedback"));
    engine.heard(&post("help"));
    let mut runs = Vec::new();
    for _ in 0..200 {
        runs = engine.runs("too-many-tasks", 0).await.unwrap();
        if !runs.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    let runs_after = engine.runs("too-many-tasks", 0).await.unwrap();
    assert_eq!(runs_after.len(), 1, "the help forum's post only");
    let ended = settled(&engine, &runs[0].id, false).await;
    let fired = ended.data.trigger.unwrap().event.unwrap();
    let created =
        post_cortex_proto::discord::PostCreated::decode(fired.payload.unwrap().value).unwrap();
    assert_eq!(created.channel, "help");
}

#[tokio::test]
async fn a_webhook_starts_its_trigger_with_its_body_as_the_input() {
    let docs = docs();
    let mut w = workflow();
    w.graph.as_mut().unwrap().blocks[0] = block(
        "start",
        block::Kind::Trigger(post_cortex_proto::workflow::Trigger {
            kind: Some(trigger::Kind::Webhook(
                post_cortex_proto::workflow::WebhookTrigger {
                    source: "hook".into(),
                },
            )),
        }),
    );
    kept(&docs, &w).await;
    let engine = engine(&docs, board(3));
    let request = |module| {
        (
            scope_of("hook", module),
            post_cortex_proto::workflow::RunWorkflowRequest {
                workflow: "too-many-tasks".into(),
                source: "hook".into(),
                body: br#"{"board": "b1", "limit": 5, "extra": true}"#.to_vec(),
                ..Default::default()
            },
        )
    };
    let (by_hand, asked) = request(None);
    let refused = engine.run(&by_hand, asked).await.err().unwrap();
    assert_eq!(
        refused.kind,
        ModuleErrorKind::FailedPrecondition,
        "no manual trigger"
    );
    let (hook, asked) = request(Some("webhooks"));
    let run = engine.run(&hook, asked).await.unwrap();
    let ended = settled(&engine, &run.id, false).await;
    assert_eq!(
        ended.data.status,
        i32::from(RunStatus::Succeeded),
        "{:?}",
        ended.data.failure
    );
    let fired = ended.data.trigger.unwrap();
    assert_eq!(fired.webhook.unwrap().source, "hook");
    assert_eq!(fired.author, "hook");
}

#[tokio::test]
async fn a_dry_run_reads_for_real_answers_its_writes_and_keeps_nothing() {
    let docs = docs();
    let board = board(3);
    let engine = engine(&docs, board.clone());
    let unkept = workflow();
    let (run, steps) = engine
        .dry_run(&scope_of("ana", None), &unkept, &input(1), false)
        .await
        .unwrap();
    assert_eq!(
        run.data.status,
        i32::from(RunStatus::Succeeded),
        "{:?}",
        run.data.failure
    );
    assert_eq!(board.paths(), [LIST], "the write is not called");
    let add = steps.iter().find(|s| s.block == "add").unwrap();
    assert_eq!(add.status, i32::from(StepStatus::Succeeded));
    assert!(add.output.is_some(), "its response's zero values");
    assert!(
        engine.runs("too-many-tasks", 0).await.unwrap().is_empty(),
        "nothing kept"
    );
    assert!(engine.ports.workflows.all().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_watched_run_answers_its_steps_then_each_change_until_it_ends() {
    let docs = docs();
    kept(&docs, &workflow()).await;
    let engine = engine(&docs, board(3));
    let run = engine
        .start(
            "too-many-tasks",
            &Start::Manual,
            Fired::default(),
            &Input::Map(input(1)),
            None,
        )
        .await
        .unwrap();
    let mut watch = engine.watch(&run.id).await.unwrap();
    let mut blocks = std::collections::BTreeSet::new();
    let mut last = None;
    let mut first = true;
    while let Some(watched) = watch.recv().await {
        match watched.unwrap() {
            Watched::Whole(run, steps) => {
                assert!(first, "whole only first: the watcher keeps up");
                blocks.extend(steps.into_iter().map(|s| s.block));
                last = Some(run.data.status);
            }
            Watched::Run(run) => last = Some(run.data.status),
            Watched::Step(step) => {
                blocks.insert(step.block.clone());
            }
        }
        first = false;
    }
    assert_eq!(last, Some(i32::from(RunStatus::Succeeded)));
    let blocks: Vec<_> = blocks.into_iter().collect();
    assert_eq!(blocks, ["add", "list", "start", "too_many"]);
}

#[tokio::test]
async fn a_workflow_is_deleted_once_no_run_of_it_is_open_and_starts_no_more() {
    let docs = docs();
    let id = kept(&docs, &workflow()).await.id;
    let board = board(3);
    let engine = engine(&docs, board.clone());
    let start = async || {
        engine
            .start(
                "too-many-tasks",
                &Start::Manual,
                Fired::default(),
                &Input::Map(input(1)),
                None,
            )
            .await
    };
    let run = start().await.unwrap();
    let workflows = engine.ports.workflows.as_ref();
    let open = engine.delete(workflows, &id).await.err().unwrap();
    assert_eq!(open.kind, ModuleErrorKind::FailedPrecondition);
    assert_eq!(workflows.all().await.unwrap().len(), 1);

    let ended = settled(&engine, &run.id, false).await;
    assert_eq!(ended.data.status, i32::from(RunStatus::Succeeded));
    engine.delete(workflows, &id).await.unwrap();
    assert!(workflows.all().await.unwrap().is_empty());
    assert!(
        !board.paths().contains(&WITHDRAW.to_string()),
        "its grants were approved: nothing waits to withdraw"
    );
    assert_eq!(start().await.err().unwrap().kind, ModuleErrorKind::NotFound);
    assert!(engine.ports.runs.get(&run.id).await.unwrap().is_some());
}

#[tokio::test]
async fn a_list_of_workflows_reads_each_ones_newest_run() {
    let docs = docs();
    kept(&docs, &workflow()).await;
    let engine = engine(&docs, board(3));
    let runs = engine.runs_port();
    assert!(runs.of("too-many-tasks", 1).await.unwrap().is_empty());
    let mut started = Vec::new();
    for _ in 0..2 {
        let run = engine
            .start(
                "too-many-tasks",
                &Start::Manual,
                Fired::default(),
                &Input::Map(input(1)),
                None,
            )
            .await
            .unwrap();
        settled(&engine, &run.id, false).await;
        started.push(run.id);
    }
    let last = runs.of("too-many-tasks", 1).await.unwrap();
    assert_eq!(
        last.iter().map(|r| &r.id).collect::<Vec<_>>(),
        [&started[1]]
    );
    let ended = runs.ended("too-many-tasks", 5).await.unwrap();
    assert_eq!(ended.len(), 2);
    assert!(runs.of("another", 5).await.unwrap().is_empty());
}
