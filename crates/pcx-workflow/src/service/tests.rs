// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use pcx_documents::InMemoryDocuments;
use post_cortex_proto::module::WorkflowTemplate;
use post_cortex_proto::workflow::{CallBlock, Field, Schema, block};
use prost_reflect::DynamicMessage;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::adapters::DocumentWorkflows;
use crate::domain::Tool;
use crate::test_support::{block, board_tools, edge, manual, set, template, text, workflow};

/// The board's tools, whoever asks.
struct Board;

#[async_trait]
impl Tools for Board {
    async fn reachable(&self, _scope: &Scope) -> Result<Vec<(String, Tool)>, ModuleError> {
        Ok(board_tools())
    }

    async fn call(
        &self,
        _scope: &Scope,
        path: &str,
        _request: &DynamicMessage,
        _idempotency_key: Option<&str>,
    ) -> Result<DynamicMessage, ModuleError> {
        Err(ModuleError::new(
            ModuleErrorKind::Unavailable,
            format!("{path} is not called here"),
        ))
    }

    async fn templates(
        &self,
        _scope: &Scope,
    ) -> Result<Vec<(String, WorkflowTemplate)>, ModuleError> {
        Ok(Vec::new())
    }
}

fn workflows() -> DocumentWorkflows {
    let docs =
        InMemoryDocuments::new(Some("ana"), post_cortex_proto::WORKFLOW_DESCRIPTOR_SET).unwrap();
    DocumentWorkflows::new(Arc::new(docs))
}

fn scope() -> Scope {
    Scope::from_wire(
        Default::default(),
        CancellationToken::new(),
        Duration::from_secs(5),
    )
}

/// A workflow `name` that calls `callee` with `board` (and a limit of 3
/// for `too-many-tasks`, which takes one).
fn caller(name: &str, callee: &str) -> Workflow {
    let mut input = vec![("board", text("{{ input.board }}"))];
    if callee == "too-many-tasks" {
        input.push(("limit", text("{{ 3 }}")));
    }
    Workflow {
        name: name.into(),
        input: Some(Schema {
            fields: vec![Field {
                name: "board".into(),
                r#type: "string".into(),
                required: true,
                ..Default::default()
            }],
        }),
        graph: Some(Graph {
            blocks: vec![
                manual(),
                block(
                    "call",
                    block::Kind::Call(CallBlock {
                        workflow: callee.into(),
                        input: Some(template(&input)),
                    }),
                ),
                set("more", "n", "steps.call + 1"),
            ],
            edges: vec![edge("start", "call", ""), edge("call", "more", "")],
            output: String::new(),
        }),
        output: "steps.more.n".into(),
        ..Default::default()
    }
}

#[tokio::test]
async fn a_workflow_is_kept_under_a_name_no_other_has() {
    let workflows = workflows();
    let service = Service::new(&workflows, &Board);
    let kept = service.create(&scope(), &workflow()).await.unwrap();
    assert_eq!(kept.version, 1);
    let again = service.create(&scope(), &workflow()).await.err().unwrap();
    assert_eq!(again.kind, ModuleErrorKind::Conflict);
    let listed = service.list().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].data.name, "too-many-tasks");
}

#[tokio::test]
async fn a_change_is_checked_and_written_from_the_version_read() {
    let workflows = workflows();
    let service = Service::new(&workflows, &Board);
    let kept = service.create(&scope(), &workflow()).await.unwrap();
    let mut changed = workflow();
    changed.description = "Now with a better description".into();
    let next = service
        .update(&scope(), &kept.id, kept.version, &changed)
        .await
        .unwrap();
    assert_eq!(next.version, 2);
    let stale = service
        .update(&scope(), &kept.id, kept.version, &changed)
        .await
        .err()
        .unwrap();
    assert_eq!(stale.kind, ModuleErrorKind::Conflict);
    changed.output = "steps.nowhere".into();
    let broken = service
        .update(&scope(), &kept.id, next.version, &changed)
        .await
        .err()
        .unwrap();
    assert_eq!(broken.kind, ModuleErrorKind::InvalidArgument);
    assert_eq!(broken.field.as_deref(), Some("output"));
}

#[tokio::test]
async fn a_call_reads_the_called_workflows_signature_and_a_cycle_is_refused() {
    let workflows = workflows();
    let service = Service::new(&workflows, &Board);
    service.create(&scope(), &workflow()).await.unwrap();
    let checked = service
        .validate(&scope(), &caller("outer", "too-many-tasks"))
        .await
        .unwrap();
    assert_eq!(checked.signature.output, pcx_expr::Type::Int);

    let unknown = service
        .validate(&scope(), &caller("outer", "nope"))
        .await
        .err()
        .unwrap();
    assert_eq!(unknown.field.as_deref(), Some("block call: call.workflow"));

    let first = service
        .create(&scope(), &caller("first", "too-many-tasks"))
        .await
        .unwrap();
    service
        .create(&scope(), &caller("second", "first"))
        .await
        .unwrap();
    let cycle = service
        .update(
            &scope(),
            &first.id,
            first.version,
            &caller("first", "second"),
        )
        .await
        .err()
        .unwrap();
    assert!(cycle.message.contains("calls itself"), "{}", cycle.message);
}

#[tokio::test]
async fn a_verdict_answers_the_rule_broken_with_its_block_and_field() {
    let workflows = workflows();
    let service = Service::new(&workflows, &Board);
    let refusal = service
        .verdict(&scope(), &caller("outer", "nope"))
        .await
        .unwrap()
        .err()
        .unwrap();
    assert_eq!(
        (refusal.block.as_deref(), refusal.field.as_str()),
        (Some("call"), "call.workflow")
    );
    assert!(
        service
            .verdict(&scope(), &workflow())
            .await
            .unwrap()
            .is_ok()
    );
}
