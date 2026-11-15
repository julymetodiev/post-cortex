// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What the tests build workflows with: the board's tools and a workflow
//! over them.

use post_cortex_proto::workflow::{
    Block, Edge, Grants, Graph, If, ManualTrigger, Schema, Set, Slot, Slots, Template, ToolBlock,
    Trigger, Value, Workflow, block, slot, trigger, value,
};
use prost_reflect::DescriptorPool;

use crate::domain::{Catalog, Tool};
use pcx_module_api::Effect;

pub const LIST: &str = "/pcx.board.v1.Boards/ListTasks";
pub const ADD: &str = "/pcx.board.v1.Boards/AddTask";
pub const DECIDE: &str = "/pcx.board.v1.Boards/Decide";
pub const GET: &str = "/pcx.board.v1.Boards/GetTask";
pub const WITHDRAW: &str = "/pcx.board.v1.Boards/WithdrawDecision";

/// The board's tools as `tools` lists them.
pub fn board_tools() -> Vec<(String, Tool)> {
    let pool = DescriptorPool::decode(post_cortex_proto::BOARD_DESCRIPTOR_SET).unwrap();
    let boards = pool.get_service_by_name("pcx.board.v1.Boards").unwrap();
    let tool = |name: &str, effect| {
        let method = boards.methods().find(|m| m.name() == name).unwrap();
        Tool {
            name: name.to_lowercase(),
            description: String::new(),
            request_schema: serde_json::Map::new(),
            response_schema: serde_json::Map::new(),
            module: "board".into(),
            effect,
            request: method.input(),
            response: method.output(),
        }
    };
    vec![
        (LIST.to_string(), tool("ListTasks", Effect::Read)),
        (ADD.to_string(), tool("AddTask", Effect::Write)),
        (DECIDE.to_string(), tool("Decide", Effect::Write)),
        (GET.to_string(), tool("GetTask", Effect::Read)),
    ]
}

/// The catalog of the board's tools.
pub fn catalog() -> Catalog {
    Catalog::new(board_tools()).unwrap()
}

pub fn block(id: &str, kind: block::Kind) -> Block {
    Block {
        id: id.into(),
        kind: Some(kind),
        ..Default::default()
    }
}

pub fn edge(from: &str, to: &str, port: &str) -> Edge {
    Edge {
        from: from.into(),
        to: to.into(),
        port: port.into(),
    }
}

pub fn text(source: &str) -> Slot {
    Slot {
        slot: Some(slot::Slot::Text(source.into())),
    }
}

pub fn template(fields: &[(&str, Slot)]) -> Template {
    Template {
        fields: fields
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.clone()))
            .collect(),
    }
}

pub fn manual() -> Block {
    block(
        "start",
        block::Kind::Trigger(Trigger {
            kind: Some(trigger::Kind::Manual(ManualTrigger {})),
        }),
    )
}

pub fn list() -> Block {
    block(
        "list",
        block::Kind::Tool(ToolBlock {
            method: LIST.into(),
            request: Some(template(&[("board", text("{{ input.board }}"))])),
            fixture: None,
        }),
    )
}

pub fn add(title: &str) -> Block {
    let labels = Slot {
        slot: Some(slot::Slot::List(Slots {
            items: vec![
                Slot {
                    slot: Some(slot::Slot::Constant(Value {
                        kind: Some(value::Kind::String("auto".into())),
                    })),
                },
                text("{{ input.board }}"),
            ],
        })),
    };
    block(
        "add",
        block::Kind::Tool(ToolBlock {
            method: ADD.into(),
            request: Some(template(&[
                ("board", text("{{ input.board }}")),
                ("title", text(title)),
                ("labels", labels),
            ])),
            fixture: None,
        }),
    )
}

pub fn set(id: &str, name: &str, source: &str) -> Block {
    block(
        id,
        block::Kind::Set(Set {
            values: [(name.to_string(), source.to_string())].into(),
        }),
    )
}

pub fn input() -> Schema {
    let field = |name: &str, ty: &str| post_cortex_proto::workflow::Field {
        name: name.into(),
        r#type: ty.into(),
        required: true,
        ..Default::default()
    };
    Schema {
        fields: vec![field("board", "string"), field("limit", "int")],
    }
}

/// start -> look, an `agent` block of `member` (empty: the host's agent),
/// with `grants`.
pub fn turn_of(member: &str, grants: &[&str]) -> Workflow {
    use post_cortex_proto::workflow::{AgentBlock, Field};
    Workflow {
        name: "daily".into(),
        graph: Some(Graph {
            blocks: vec![
                manual(),
                block(
                    "look",
                    block::Kind::Agent(AgentBlock {
                        member: member.into(),
                        prompt: "Is anything firing?".into(),
                        output: Some(Schema {
                            fields: vec![Field {
                                name: "report".into(),
                                r#type: "string".into(),
                                required: true,
                                ..Default::default()
                            }],
                        }),
                    }),
                ),
            ],
            edges: vec![edge("start", "look", "")],
            output: String::new(),
        }),
        grants: Some(Grants {
            modules: grants.iter().map(|m| (*m).to_string()).collect(),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// start -> list -> too_many? then add, else count; its output the number
/// of tasks.
pub fn workflow() -> Workflow {
    Workflow {
        name: "too-many-tasks".into(),
        description: "Adds a task when a board has too many".into(),
        input: Some(input()),
        graph: Some(Graph {
            blocks: vec![
                manual(),
                list(),
                block(
                    "too_many",
                    block::Kind::Branch(If {
                        condition: "size(steps.list.tasks) > input.limit".into(),
                    }),
                ),
                add("Too many: {{ size(steps.list.tasks) }} on {{ trigger.author }}"),
                set("count", "n", "size(steps.list.tasks)"),
            ],
            edges: vec![
                edge("start", "list", ""),
                edge("list", "too_many", "next"),
                edge("too_many", "add", "then"),
                edge("too_many", "count", "else"),
            ],
            output: String::new(),
        }),
        output: "size(steps.list.tasks)".into(),
        grants: Some(Grants {
            modules: vec!["board".into()],
            ..Default::default()
        }),
        ..Default::default()
    }
}
