// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The RPCs of `tools-shell` as tools (W23), for agents and people alike.

use post_cortex_proto::module::{Effect, Rpc};

/// The module's service.
pub const SERVICE: &str = "pcx.tools.shell.v1.Shell";

/// Each RPC of the service and its tool name.
const RPCS: [(&str, &str, Effect); 6] = [
    ("Bash", "bash", Effect::External),
    ("TmuxStart", "tmux_start", Effect::External),
    ("TmuxRead", "tmux_read", Effect::Read),
    ("TmuxSend", "tmux_send", Effect::External),
    ("TmuxList", "tmux_list", Effect::Read),
    ("TmuxKill", "tmux_kill", Effect::External),
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
