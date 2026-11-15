// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use post_cortex_proto::board::Risk;
use post_cortex_proto::workflow::{
    Approval, Block, CallBlock, Edge, EventTrigger, ForEach, Grants, Graph, Point, Trigger, block,
    trigger,
};

use super::*;
use crate::test_support::{add, block, catalog, edge, set, template, text, turn_of, workflow};

fn refused(workflow: &Workflow) -> Refusal {
    check(workflow, &catalog(), &BTreeMap::new()).err().unwrap()
}

fn blocks(workflow: &mut Workflow) -> &mut Vec<Block> {
    &mut workflow.graph.as_mut().unwrap().blocks
}

fn edges(workflow: &mut Workflow) -> &mut Vec<Edge> {
    &mut workflow.graph.as_mut().unwrap().edges
}

#[test]
fn a_workflow_checks_against_its_tools_messages_and_gives_its_signature() {
    let checked = check(&workflow(), &catalog(), &BTreeMap::new()).unwrap();
    assert_eq!(checked.signature.output, pcx_expr::Type::Int);
    assert_eq!(checked.signature.input.len(), 2);
    let tools: Vec<_> = checked.found.tools.keys().map(String::as_str).collect();
    assert_eq!(tools, ["add", "list"]);
    assert_eq!(checked.found.tools["add"].effect, Effect::Write);
}

#[test]
fn a_field_of_another_type_or_no_field_of_the_request_is_refused_by_name() {
    let mut w = workflow();
    blocks(&mut w)[3] = add("{{ input.limit }}");
    let r = refused(&w);
    assert_eq!(
        (r.block.as_deref(), r.field.as_str()),
        (Some("add"), "tool.request.title")
    );
    assert!(r.message.contains("int"), "{r}");

    let mut w = workflow();
    let block::Kind::Tool(tool) = blocks(&mut w)[1].kind.as_mut().unwrap() else {
        unreachable!()
    };
    tool.request = Some(template(&[("bord", text("x"))]));
    let r = refused(&w);
    assert_eq!(r.field, "tool.request.bord");
}

#[test]
fn a_step_not_on_every_path_is_not_there() {
    let mut w = workflow();
    w.output = "steps.add.id".into();
    let r = refused(&w);
    assert_eq!((r.block, r.field.as_str()), (None, "output"));
    let mut w = workflow();
    blocks(&mut w)[4] = set("count", "n", "steps.add.id");
    assert_eq!(refused(&w).block.as_deref(), Some("count"));
}

#[test]
fn the_graph_has_its_triggers_as_starts_no_cycles_and_its_edges_ends() {
    let mut w = workflow();
    edges(&mut w).push(edge("count", "list", ""));
    assert!(refused(&w).message.contains("cycle"), "{}", refused(&w));

    let mut w = workflow();
    edges(&mut w).push(edge("count", "nowhere", ""));
    assert!(refused(&w).message.contains("nowhere"));

    let mut w = workflow();
    edges(&mut w)[2].port = "next".into();
    assert!(refused(&w).message.contains("no port"));

    let mut w = workflow();
    edges(&mut w).remove(0);
    assert_eq!(refused(&w).block.as_deref(), Some("list"));

    let mut w = workflow();
    blocks(&mut w)[4].id = "list".into();
    assert!(refused(&w).message.contains("another block"));

    let mut w = workflow();
    blocks(&mut w).remove(0);
    edges(&mut w).remove(0);
    assert!(refused(&w).message.contains("trigger"));
}

#[test]
fn the_grants_are_the_authors_modules_and_reach_every_tool() {
    let mut w = workflow();
    w.grants = Some(Grants::default());
    let r = refused(&w);
    assert_eq!(
        (r.block.as_deref(), r.field.as_str()),
        (Some("list"), "tool.method")
    );
    let mut w = workflow();
    w.grants = Some(Grants {
        modules: vec!["board".into(), "ops".into()],
        ..Default::default()
    });
    assert_eq!(refused(&w).field, "grants.modules[1]");
}

#[test]
fn a_turn_of_the_hosts_agent_needs_a_module_in_the_grants_and_a_members_none() {
    // The agent's own turn is given the tools of the grants' modules: with
    // none it would have every tool.
    let r = refused(&turn_of("", &[]));
    assert_eq!(
        (r.block.as_deref(), r.field.as_str()),
        (Some("look"), "agent.member")
    );
    assert!(check(&turn_of("", &["board"]), &catalog(), &BTreeMap::new()).is_ok());
    // A member's turn has the member's own tools.
    assert!(check(&turn_of("eq-sentinel", &[]), &catalog(), &BTreeMap::new()).is_ok());
}

#[test]
fn a_for_each_body_reads_its_item_and_gives_a_list_of_its_outputs() {
    let mut w = workflow();
    let each = block(
        "each",
        block::Kind::ForEach(ForEach {
            items: "steps.list.tasks".into(),
            concurrency: 4,
            body: Some(Graph {
                blocks: vec![set("pick", "title", "item.task.title + string(index)")],
                edges: vec![],
                output: "steps.pick.title".into(),
            }),
        }),
    );
    blocks(&mut w)[4] = each;
    edges(&mut w)[3].to = "each".into();
    w.output = String::new();
    blocks(&mut w).push(set("first", "t", "steps.each[0]"));
    edges(&mut w).push(edge("each", "first", ""));
    let checked = check(&w, &catalog(), &BTreeMap::new()).unwrap();
    assert_eq!(checked.signature.output, pcx_expr::Type::Null);

    let block::Kind::Set(first) = blocks(&mut w)[5].kind.as_mut().unwrap() else {
        unreachable!()
    };
    first.values.insert("t".into(), "steps.each[0] + 1".into());
    assert_eq!(refused(&w).block.as_deref(), Some("first"));
}

#[test]
fn an_error_edge_gives_the_failure() {
    let mut w = workflow();
    blocks(&mut w).push(set(
        "failed",
        "why",
        "steps.list.kind + ': ' + steps.list.message",
    ));
    edges(&mut w).push(edge("list", "failed", "error"));
    // The run ends at `failed` too, where steps.list is the failure: the
    // output's steps.list.tasks is not on every end.
    assert_eq!(refused(&w).field, "output");
    w.output = String::new();
    check(&w, &catalog(), &BTreeMap::new()).unwrap();
}

#[test]
fn a_call_fills_the_called_workflows_required_input() {
    let callee = Signature {
        input: vec![Field {
            name: "board".into(),
            ty: pcx_expr::Type::String,
            required: true,
            example: None,
            constraints: pcx_expr::constraints::Constraints::default(),
        }],
        output: pcx_expr::Type::Int,
    };
    let callees = BTreeMap::from([("other".to_string(), callee)]);
    let mut w = workflow();
    blocks(&mut w)[4] = block(
        "count",
        block::Kind::Call(CallBlock {
            workflow: "other".into(),
            input: Some(template(&[])),
        }),
    );
    w.output = String::new();
    let r = check(&w, &catalog(), &callees).err().unwrap();
    assert_eq!(r.field, "call.input.board");
    let block::Kind::Call(call) = blocks(&mut w)[4].kind.as_mut().unwrap() else {
        unreachable!()
    };
    call.input = Some(template(&[("board", text("{{ input.board }}"))]));
    check(&w, &catalog(), &callees).unwrap();
}

#[test]
fn an_event_triggers_filter_reads_the_event_and_an_approval_needs_a_risk() {
    let mut w = workflow();
    blocks(&mut w)[0] = block(
        "start",
        block::Kind::Trigger(Trigger {
            kind: Some(trigger::Kind::Event(EventTrigger {
                name: "board.task_moved".into(),
                filter: "event.name == 'board.task_moved' && event.workspace_id != ''".into(),
            })),
        }),
    );
    check(&w, &catalog(), &BTreeMap::new()).unwrap();
    let block::Kind::Trigger(t) = blocks(&mut w)[0].kind.as_mut().unwrap() else {
        unreachable!()
    };
    t.kind = Some(trigger::Kind::Event(EventTrigger {
        name: "board.task_moved".into(),
        filter: "event.nope".into(),
    }));
    assert_eq!(refused(&w).field, "trigger.event.filter");

    let mut w = workflow();
    blocks(&mut w)[4] = block(
        "count",
        block::Kind::Approval(Approval {
            question: "Add {{ size(steps.list.tasks) }} tasks?".into(),
            risk: Risk::None.into(),
            task: String::new(),
            person: false,
        }),
    );
    assert_eq!(refused(&w).field, "approval.risk");
}

/// The JSON blocks of `text` (a Markdown file), in order.
fn json_blocks(text: &str) -> Vec<serde_json::Value> {
    text.split("```json\n")
        .skip(1)
        .map(|rest| serde_json::from_str(rest.split("```").next().unwrap()).unwrap())
        .collect()
}

/// `json` read as message `name` of `descriptors`, then as `T`.
fn read_as<T: prost::Message + Default>(
    descriptors: &[u8],
    name: &str,
    json: serde_json::Value,
) -> T {
    let pool = DescriptorPool::decode(descriptors).unwrap();
    let message =
        prost_reflect::DynamicMessage::deserialize(pool.get_message_by_name(name).unwrap(), json)
            .unwrap();
    message.transcode_to::<T>().unwrap()
}

#[test]
fn the_builder_skills_example_reads_as_its_tools_take_it_and_checks() {
    let skill = include_str!("../../../pcx-skills/library/workflow-builder.md");
    let blocks = json_blocks(skill);
    assert_eq!(
        blocks.len(),
        5,
        "the workflow's shape, an http block, the example, its job, the data example"
    );
    let tool = |descriptors: &[u8], service: &str, method: &str, module: &str, effect| {
        let pool = DescriptorPool::decode(descriptors).unwrap();
        let found = pool
            .get_service_by_name(service)
            .unwrap()
            .methods()
            .find(|m| m.name() == method)
            .unwrap();
        (
            format!("/{service}/{method}"),
            Tool {
                name: method.to_lowercase(),
                description: String::new(),
                request_schema: serde_json::Map::new(),
                response_schema: serde_json::Map::new(),
                module: module.to_string(),
                effect,
                request: found.input(),
                response: found.output(),
            },
        )
    };
    let catalog = Catalog::new([
        tool(
            post_cortex_proto::BOARD_DESCRIPTOR_SET,
            "pcx.board.v1.Boards",
            "ListTasks",
            "board",
            pcx_module_api::Effect::Read,
        ),
        tool(
            post_cortex_proto::MAIL_DESCRIPTOR_SET,
            "pcx.mail.v1.Mail",
            "Send",
            "mail",
            pcx_module_api::Effect::External,
        ),
        tool(
            post_cortex_proto::HTTP_DESCRIPTOR_SET,
            "pcx.http.v1.Http",
            "Get",
            "http",
            pcx_module_api::Effect::Read,
        ),
    ])
    .unwrap();
    // The http block checks in a workflow that gives it its input.
    let around = serde_json::json!({
        "name": "repo",
        "input": {"fields": [
            {"name": "owner", "type": "string", "required": true},
            {"name": "repo", "type": "string", "required": true}
        ]},
        "graph": {
            "blocks": [{"id": "start", "trigger": {"manual": {}}}, blocks[1].clone()],
            "edges": [{"from": "start", "to": "repo", "port": "next"}]
        },
        "grants": {"modules": ["http"]}
    });
    let http: Workflow = read_as(
        post_cortex_proto::WORKFLOW_DESCRIPTOR_SET,
        "pcx.workflow.v1.Workflow",
        around,
    );
    check(&http, &catalog, &BTreeMap::new()).unwrap();
    let example: Workflow = read_as(
        post_cortex_proto::WORKFLOW_DESCRIPTOR_SET,
        "pcx.workflow.v1.Workflow",
        blocks[2].clone(),
    );
    check(&example, &catalog, &BTreeMap::new()).unwrap();
    let job: post_cortex_proto::scheduler::AddJobRequest = read_as(
        post_cortex_proto::SCHEDULER_DESCRIPTOR_SET,
        "pcx.scheduler.v1.AddJobRequest",
        blocks[3].clone(),
    );
    assert!(matches!(
        job.job.unwrap().does,
        Some(post_cortex_proto::scheduler::job::Does::Workflow(_))
    ));
    let data: Workflow = read_as(
        post_cortex_proto::WORKFLOW_DESCRIPTOR_SET,
        "pcx.workflow.v1.Workflow",
        blocks[4].clone(),
    );
    let with_records = catalog.with_records(&data).unwrap();
    check(&data, &with_records, &BTreeMap::new()).unwrap();
}

#[test]
fn the_layout_places_only_blocks_of_the_workflow() {
    let mut w = workflow();
    w.layout.insert("add".into(), Point { x: 40.0, y: 120.0 });
    assert!(check(&w, &catalog(), &BTreeMap::new()).is_ok());
    w.layout.insert("gone".into(), Point { x: 0.0, y: 0.0 });
    let r = refused(&w);
    assert_eq!((r.block, r.field.as_str()), (None, "layout.gone"));
    w.layout.remove("gone");
    w.layout.insert(
        "list".into(),
        Point {
            x: f64::NAN,
            y: 0.0,
        },
    );
    assert_eq!(refused(&w).field, "layout.list");
}
