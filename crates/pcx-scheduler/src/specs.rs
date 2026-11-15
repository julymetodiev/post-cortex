// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The scheduler's RPCs as tools (W28), for agents and people alike, and
//! its document kinds as its manifest declares them.

use pcx_module_api::DocumentKind;
use post_cortex_proto::module::{Effect, Rpc};
use post_cortex_proto::scheduler::{Job, Run};
use prost::Name;

use crate::domain::Kind;

/// The scheduler's service.
pub const SERVICE: &str = "pcx.scheduler.v1.Scheduler";

/// Each RPC of the service and its tool name.
const RPCS: [(&str, &str, Effect); 7] = [
    ("AddJob", "job_add", Effect::Write),
    ("UpdateJob", "job_update", Effect::Write),
    ("ListJobs", "job_list", Effect::Read),
    ("RemoveJob", "job_remove", Effect::Write),
    ("RunJob", "job_run", Effect::External),
    ("ListRuns", "run_list", Effect::Read),
    ("JobHealth", "job_health", Effect::Read),
];

/// Every RPC of the module, as its manifest names it.
pub fn rpcs() -> Vec<Rpc> {
    RPCS.iter()
        .map(|(method, tool, effect)| Rpc {
            method: format!("/{SERVICE}/{method}"),
            tool: (*tool).to_string(),
            visibility: Vec::new(),
            effect: (*effect).into(),
        })
        .collect()
}

/// Its kinds: a job titled by its name, a run by its job's name.
pub fn document_kinds() -> Vec<DocumentKind> {
    Kind::ALL
        .iter()
        .map(|kind| DocumentKind {
            name: kind.name().to_string(),
            title_field: "name".to_string(),
            searchable: None,
            r#type: match kind {
                Kind::Job => Job::full_name(),
                Kind::Run => Run::full_name(),
            },
        })
        .collect()
}
