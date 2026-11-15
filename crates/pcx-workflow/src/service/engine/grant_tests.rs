// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

// The grants of a workflow (docs/modules/workflow.md, triggers and
// approval): asked on the board when it is written, all or nothing, asked
// again at start when they never were, and approved meanwhile read again.

use super::*;
use crate::service::engine::test_engine::{
    approved, board, docs, engine, input, read, scope_of, settled,
};
use crate::test_support::{ADD, DECIDE, GET, WITHDRAW, workflow};

#[tokio::test]
async fn a_workflow_deleted_while_its_grants_wait_withdraws_their_decision() {
    let docs = docs();
    let board = board(3);
    let engine = engine(&docs, board.clone());
    let workflows = crate::adapters::DocumentWorkflows::new(docs.clone());
    let created = engine
        .create(&scope_of("ana", None), &workflows, &workflow())
        .await
        .unwrap();
    assert!(created.data.asked.is_some());
    engine.delete(&workflows, &created.id).await.unwrap();
    assert_eq!(board.paths(), [ADD, DECIDE, WITHDRAW]);
}

#[tokio::test]
async fn a_new_workflow_runs_once_a_person_approves_its_grants() {
    let docs = docs();
    let board = board(3);
    let engine = engine(&docs, board.clone());
    let workflows = crate::adapters::DocumentWorkflows::new(docs.clone());
    let ana = scope_of("ana", None);
    let mut asked = workflow();
    asked.approved = Some(post_cortex_proto::workflow::Approved::default());
    let created = engine.create(&ana, &workflows, &asked).await.unwrap();
    assert_eq!(board.paths(), [ADD, DECIDE]);
    let key = board.calls.lock()[0].1.clone();
    assert_eq!(key, None, "a task's key is in its request, not the call's");
    let waiting = created.data.asked.clone().unwrap();
    assert_eq!(
        (waiting.task.as_str(), waiting.decision.as_str()),
        ("task-1", "decision-1")
    );
    assert!(
        created.data.approved.is_none(),
        "the request approves nothing"
    );

    let refused = engine
        .start(
            "too-many-tasks",
            &Start::Manual,
            Fired::default(),
            &Input::Map(input(1)),
            None,
        )
        .await
        .err()
        .unwrap();
    assert_eq!(refused.kind, ModuleErrorKind::FailedPrecondition);

    engine.decided(&approved("ana")).await.unwrap();
    let ok = read(&engine, "too-many-tasks").await;
    assert_eq!(ok.approved.unwrap().approved_by, "ana");
    assert!(ok.asked.is_none());
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
    assert_eq!(ended.data.status, i32::from(RunStatus::Succeeded));
}

#[tokio::test]
async fn an_approval_made_while_the_module_was_down_is_read_again() {
    let docs = docs();
    let board = board(3);
    let first = engine(&docs, board.clone());
    let workflows = crate::adapters::DocumentWorkflows::new(docs.clone());
    first
        .create(&scope_of("ana", None), &workflows, &workflow())
        .await
        .unwrap();
    *board.approved.lock() = Some("ana".into());
    let second = engine(&docs, board.clone());
    second.resume().await.unwrap();
    let approved = read(&second, "too-many-tasks").await.approved.unwrap();
    assert_eq!(approved.approved_by, "ana");
}

#[tokio::test]
async fn a_new_workflow_whose_grants_are_refused_is_not_kept_and_one_unasked_is_asked_at_start() {
    let docs = docs();
    let board = board(3);
    let engine = engine(&docs, board.clone());
    let workflows = crate::adapters::DocumentWorkflows::new(docs.clone());
    let ana = scope_of("ana", None);
    *board.refuses.lock() = true;
    let refused = engine.create(&ana, &workflows, &workflow()).await;
    assert!(refused.is_err());
    assert!(workflows.all().await.unwrap().is_empty(), "all or nothing");

    // One kept without its question (written before this rule): the next
    // start asks it.
    workflows.create(&workflow()).await.unwrap();
    *board.refuses.lock() = false;
    engine.recheck().await.unwrap();
    let asked = workflows.all().await.unwrap().remove(0);
    assert!(asked.data.asked.is_some());
}

#[tokio::test]
async fn a_workflow_is_written_only_when_what_its_agent_blocks_ask_is_there() {
    // Prod 2026-11-14: a template installed, and its first run failed with
    // "no member community in the workspace's teams".
    use crate::service::engine::test_engine::{Unused, engine_with};
    use crate::test_support::turn_of;

    let docs = docs();
    let board = board(3);
    let world = Arc::new(Unused::default());
    world.absent.lock().push("community".into());
    let engine = engine_with(&docs, board.clone(), world.clone());
    let workflows = crate::adapters::DocumentWorkflows::new(docs.clone());
    let ana = scope_of("ana", None);

    let refused = engine
        .create(&ana, &workflows, &turn_of("community", &[]))
        .await
        .unwrap_err();
    assert_eq!(refused.kind, ModuleErrorKind::FailedPrecondition);
    assert_eq!(refused.field.as_deref(), Some("agent.member"));
    assert!(refused.message.contains("no member community"));
    assert!(workflows.all().await.unwrap().is_empty(), "nothing is kept");
    assert!(board.paths().is_empty(), "and no one is asked");

    // Once the member is in a team it is written, and asked for.
    world.absent.lock().clear();
    let kept = engine
        .create(&ana, &workflows, &turn_of("community", &[]))
        .await
        .unwrap();
    assert!(kept.data.asked.is_some());
}

/// The board's template `lister`: it lists the tasks of a board, its
/// description naming `${what}`.
fn lister() -> post_cortex_proto::module::WorkflowTemplate {
    post_cortex_proto::module::WorkflowTemplate {
        name: "lister".into(),
        description: "Lists a board's tasks".into(),
        workflow: r#"{
            "description": "Lists ${what}",
            "input": {"fields": [{"name": "board", "type": "string", "required": true}]},
            "graph": {
                "blocks": [
                    {"id": "start", "trigger": {"manual": {}}},
                    {"id": "list", "tool": {
                        "method": "/pcx.board.v1.Boards/ListTasks",
                        "request": {"fields": {"board": {"text": "{{ input.board }}"}}}
                    }}
                ],
                "edges": [{"from": "start", "to": "list"}]
            },
            "grants": {"modules": ["board"]}
        }"#
        .into(),
        parameters: vec![post_cortex_proto::module::TemplateParameter {
            name: "what".into(),
            description: "What it lists".into(),
            default: "tasks".into(),
            pattern: "[a-z ]{1,32}".into(),
        }],
    }
}

#[tokio::test]
async fn a_template_installed_is_a_new_workflow_whose_grants_are_asked_for() {
    let docs = docs();
    let board = board(3);
    board.templates.lock().push(("board".to_string(), lister()));
    let engine = engine(&docs, board.clone());
    let workflows = crate::adapters::DocumentWorkflows::new(docs.clone());
    let ana = scope_of("ana", None);
    let service = Service::new(&workflows, engine.ports.tools.as_ref());
    let listed = service.templates(&ana, "").await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(
        (listed[0].module.as_str(), listed[0].installed.as_str()),
        ("board", "")
    );
    assert!(service.templates(&ana, "discord").await.unwrap().is_empty());

    let values = [("what".to_string(), "open tasks".to_string())].into();
    let kept = engine
        .install(&ana, &workflows, "board", "lister", &values)
        .await
        .unwrap();
    assert_eq!(kept.data.name, "lister");
    assert_eq!(kept.data.description, "Lists open tasks");
    assert!(
        kept.data.asked.is_some(),
        "its grants are asked as any new one's"
    );
    assert_eq!(board.paths(), [ADD, DECIDE]);
    let listed = service.templates(&ana, "board").await.unwrap();
    assert_eq!(listed[0].installed, "lister");

    let again = engine
        .install(&ana, &workflows, "board", "lister", &values)
        .await
        .unwrap_err();
    assert_eq!(
        again.kind,
        ModuleErrorKind::Conflict,
        "installed once under its name"
    );
    let none = engine
        .install(&ana, &workflows, "discord", "lister", &values)
        .await
        .unwrap_err();
    assert_eq!(none.kind, ModuleErrorKind::NotFound);
}

/// Mail's `Send`, of effect `external`, as the host lists it.
fn mail_send() -> (String, crate::domain::Tool) {
    let pool =
        prost_reflect::DescriptorPool::decode(post_cortex_proto::MAIL_DESCRIPTOR_SET).unwrap();
    let method = pool
        .get_service_by_name("pcx.mail.v1.Mail")
        .unwrap()
        .methods()
        .find(|m| m.name() == "Send")
        .unwrap();
    (
        "/pcx.mail.v1.Mail/Send".to_string(),
        crate::domain::Tool {
            name: "mail_send".into(),
            description: String::new(),
            request_schema: serde_json::Map::new(),
            response_schema: serde_json::Map::new(),
            module: "mail".into(),
            effect: pcx_module_api::Effect::External,
            request: method.input(),
            response: method.output(),
        },
    )
}

#[tokio::test]
async fn a_workflow_approved_before_its_actions_were_asked_for_is_asked_again_at_start() {
    use crate::test_support::{block, edge, manual, template, text};
    use post_cortex_proto::workflow::{Approved, Grants, Graph, ToolBlock, block::Kind};

    let docs = docs();
    let board = board(3);
    board.others.lock().push(mail_send());
    let engine = engine(&docs, board.clone());
    // As it was kept before tools were asked for: its modules approved,
    // no tool named.
    let old = Workflow {
        name: "mailer".into(),
        graph: Some(Graph {
            blocks: vec![
                manual(),
                block(
                    "send",
                    Kind::Tool(ToolBlock {
                        method: "/pcx.mail.v1.Mail/Send".into(),
                        request: Some(template(&[
                            ("subject", text("Hi")),
                            ("text", text("Hello")),
                        ])),
                        fixture: None,
                    }),
                ),
            ],
            edges: vec![edge("start", "send", "")],
            output: String::new(),
        }),
        grants: Some(Grants {
            modules: vec!["mail".into()],
            ..Default::default()
        }),
        approved: Some(Approved {
            grants: Some(Grants {
                modules: vec!["mail".into()],
                ..Default::default()
            }),
            decision: "d-0".into(),
            approved_by: "ana".into(),
        }),
        ..Default::default()
    };
    crate::adapters::DocumentWorkflows::new(docs.clone())
        .create(&old)
        .await
        .unwrap();

    let refused = engine
        .start(
            "mailer",
            &Start::Manual,
            Fired::default(),
            &Input::Map(Default::default()),
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(
        refused.kind,
        ModuleErrorKind::FailedPrecondition,
        "a run is judged on the actions its graph calls"
    );

    engine.recheck().await.unwrap();
    let asked = read(&engine, "mailer").await;
    assert_eq!(asked.grants.unwrap().tools, ["mail_send"]);
    assert_eq!(asked.asked.unwrap().grants.unwrap().tools, ["mail_send"]);
    assert_eq!(board.paths(), [ADD, DECIDE]);
    let question: post_cortex_proto::board::DecideRequest =
        board.decided.lock()[0].transcode_to().unwrap();
    assert_eq!(
        question.what,
        "Let workflow mailer also act with mail_send (it calls mail now)?"
    );

    // Asked again at the next start, with nothing changed: not asked twice.
    engine.recheck().await.unwrap();
    assert_eq!(board.paths(), [ADD, DECIDE, GET]);
}
