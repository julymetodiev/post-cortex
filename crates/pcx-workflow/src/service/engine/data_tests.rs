// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Data blocks through the engine: their records in the run's catalog, a
//! `validate`'s ports and failure, and a preview over a dry run's steps.

use pcx_expr::Value as Expr;
use post_cortex_proto::workflow::{
    Field, PreviewRequest, Rule, Schema, TransformBlock, ValidateBlock, block, transform_block,
    validate_block,
};

use super::*;
use crate::service::engine::test_engine::{board, docs, engine, input, kept, scope_of, settled};
use crate::test_support::{block, catalog, edge, list, manual, set, workflow};

/// start -> list -> `t` (a record of each task: its id and its number) ->
/// `v` (each number below 2), then `good` by `valid`, `bad` by `invalid`
/// when `invalid` has an edge.
fn tasks(invalid_edge: bool) -> Workflow {
    let field = |name: &str, ty: &str| Field {
        name: name.into(),
        r#type: ty.into(),
        required: true,
        ..Default::default()
    };
    let t = TransformBlock {
        input: Some(transform_block::Input::Items("steps.list.tasks".into())),
        output: Some(Schema {
            fields: vec![field("id", "string"), field("n", "int")],
        }),
        fields: [
            ("id".to_string(), "item.id".to_string()),
            ("n".to_string(), "int(item.id.substring(1))".to_string()),
        ]
        .into(),
        ..Default::default()
    };
    let v = ValidateBlock {
        input: Some(validate_block::Input::Items("steps.t.items".into())),
        schema: None,
        rules: vec![Rule {
            holds: "item.n < 2".into(),
            message: "{{ item.id }} is too far".into(),
            path: "n".into(),
        }],
    };
    let mut edges = vec![
        edge("start", "list", ""),
        edge("list", "t", ""),
        edge("t", "v", ""),
        edge("v", "good", "valid"),
    ];
    let mut blocks = vec![
        manual(),
        list(),
        block("t", block::Kind::Transform(t)),
        block("v", block::Kind::Validate(v)),
        set("good", "x", "steps.t.items[0].id"),
    ];
    if invalid_edge {
        edges.push(edge("v", "bad", "invalid"));
        blocks.push(set("bad", "x", "steps.v.invalid[0].index"));
    }
    Workflow {
        name: if invalid_edge {
            "far-tasks"
        } else {
            "near-tasks"
        }
        .into(),
        graph: Some(post_cortex_proto::workflow::Graph {
            blocks,
            edges,
            output: String::new(),
        }),
        output: "size(steps.v.violations)".into(),
        ..workflow()
    }
}

async fn ran(w: &Workflow) -> (Kept<Run>, Arc<Engine>) {
    let docs = docs();
    kept(&docs, w).await;
    let engine = engine(&docs, board(3));
    let run = engine
        .start(
            &w.name,
            &Start::Manual,
            Fired::default(),
            &Input::Map(input(1)),
            None,
        )
        .await
        .unwrap();
    (settled(&engine, &run.id, false).await, engine)
}

#[tokio::test]
async fn a_validate_with_an_invalid_edge_takes_it_with_its_violations() {
    let w = tasks(true);
    let (ended, engine) = ran(&w).await;
    assert_eq!(
        ended.data.status,
        i32::from(RunStatus::Succeeded),
        "{:?}",
        ended.data.failure
    );
    let output =
        domain::read_value(ended.data.output.as_ref().unwrap(), catalog().messages()).unwrap();
    assert_eq!(output, Expr::Int(1));
    let steps = engine.ports.runs.steps(&ended.id).await.unwrap();
    let v = steps.iter().find(|s| s.data.block == "v").unwrap();
    assert_eq!(v.data.port, "invalid");
    assert!(steps.iter().any(|s| s.data.block == "bad"));
}

#[tokio::test]
async fn a_validate_without_an_invalid_edge_fails_the_run_with_kind_invalid() {
    let (ended, _) = ran(&tasks(false)).await;
    assert_eq!(ended.data.status, i32::from(RunStatus::Failed));
    let failure = ended.data.failure.unwrap();
    assert_eq!(failure.kind, "invalid");
    assert!(
        failure.message.contains("items[2].n: t2 is too far"),
        "{}",
        failure.message
    );
}

#[tokio::test]
async fn a_data_block_is_previewed_over_a_dry_runs_steps() {
    let docs = docs();
    let engine = engine(&docs, board(3));
    let w = tasks(true);
    let scope = scope_of("ana", None);
    let (_, steps) = engine.dry_run(&scope, &w, &input(1), false).await.unwrap();
    let preview = engine
        .preview(
            &scope,
            w,
            PreviewRequest {
                block: "v".into(),
                input: Some(input(1)),
                steps,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!((preview.port.as_str(), preview.items), ("invalid", 3));
    assert!(preview.failure.is_none());
    let output = preview.output.unwrap();
    assert!(output.entries.contains_key("violations"));
}
