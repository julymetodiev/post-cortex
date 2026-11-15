// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The RPCs of `tools-fs` as tools (W23), for agents and people alike.

use post_cortex_proto::module::{Effect, Rpc};

/// The module's service.
pub const SERVICE: &str = "pcx.tools.fs.v1.Files";

/// Each RPC of the service and its tool name.
const RPCS: [(&str, &str, Effect); 6] = [
    ("Read", "read", Effect::Read),
    ("Write", "write", Effect::Write),
    ("Edit", "edit", Effect::Write),
    ("ListDir", "list_dir", Effect::Read),
    ("Glob", "glob", Effect::Read),
    ("Grep", "grep", Effect::Read),
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
