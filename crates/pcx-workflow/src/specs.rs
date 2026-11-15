// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The workflow module's RPCs as tools (docs/modules/workflow.md, tools),
//! the events it emits, and its document kinds as its manifest declares
//! them.

use pcx_module_api::DocumentKind;
use post_cortex_proto::module::{Effect, EmittedEvent, Rpc, ToolVisibility};
use post_cortex_proto::workflow::{Run, RunEnded, StateEntry, Step, Workflow};
use prost::Name;

use crate::domain::{RUN_KIND, STATE_KIND, STEP_KIND, WORKFLOW_KIND};
use crate::service::engine::events;

/// The module's service.
pub const SERVICE: &str = "pcx.workflow.v1.Workflows";

/// Each RPC of the service, its tool name and its effect: a run may do
/// anything its grants allow (`external`), the writes are `write`, a dry
/// run only reads.
const RPCS: [(&str, &str, Effect); 13] = [
    ("CreateWorkflow", "workflow_create", Effect::Write),
    ("UpdateWorkflow", "workflow_update", Effect::Write),
    ("DeleteWorkflow", "workflow_delete", Effect::Write),
    ("ValidateWorkflow", "workflow_validate", Effect::Read),
    ("ListWorkflows", "workflow_list", Effect::Read),
    ("ListTools", "workflow_tools", Effect::Read),
    ("ListTemplates", "workflow_templates", Effect::Read),
    ("InstallTemplate", "workflow_install", Effect::Write),
    ("RunWorkflow", "workflow_run", Effect::External),
    ("DryRunWorkflow", "workflow_dry_run", Effect::Read),
    ("ListRuns", "workflow_runs", Effect::Read),
    ("GetRun", "workflow_run_get", Effect::Read),
    ("CancelRun", "workflow_cancel", Effect::Write),
];

/// The RPCs only the app calls, not tools (the editor): a run followed as
/// it goes, and a data block previewed over a run's steps.
const APP_ONLY: [&str; 2] = ["WatchRun", "PreviewBlock"];

/// Every RPC of the module, as its manifest names it.
pub fn rpcs() -> Vec<Rpc> {
    RPCS.iter()
        .map(|(method, tool, effect)| Rpc {
            method: format!("/{SERVICE}/{method}"),
            tool: (*tool).to_string(),
            visibility: Vec::new(),
            effect: (*effect).into(),
        })
        .chain(APP_ONLY.iter().map(|method| Rpc {
            method: format!("/{SERVICE}/{method}"),
            tool: String::new(),
            visibility: vec![ToolVisibility::App.into()],
            effect: Effect::Read.into(),
        }))
        .collect()
}

/// The events it emits.
pub fn emits() -> Vec<EmittedEvent> {
    [
        (events::SUCCEEDED, "A run of a workflow succeeded."),
        (
            events::FAILED,
            "A run of a workflow failed or was cancelled.",
        ),
        (events::FAILING, "Three runs of a workflow failed in a row."),
    ]
    .map(|(name, description)| EmittedEvent {
        name: name.to_string(),
        r#type: RunEnded::full_name(),
        description: description.to_string(),
    })
    .to_vec()
}

/// Its kinds: a workflow titled by its name and searched; its runs, their
/// steps and its store kept out of search.
pub fn document_kinds() -> Vec<DocumentKind> {
    let kind = |name: &str, title: &str, searchable: Option<bool>, ty: String| DocumentKind {
        name: name.to_string(),
        title_field: title.to_string(),
        searchable,
        r#type: ty,
    };
    vec![
        kind(WORKFLOW_KIND, "name", None, Workflow::full_name()),
        kind(RUN_KIND, "workflow", Some(false), Run::full_name()),
        kind(STEP_KIND, "block", Some(false), Step::full_name()),
        kind(STATE_KIND, "key", Some(false), StateEntry::full_name()),
    ]
}
