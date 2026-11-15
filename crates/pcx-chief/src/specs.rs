// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The chief's RPCs as tools (W23): `Ask` for any caller, the rest for
//! its view only.

use post_cortex_proto::module::{Effect, Rpc, ToolVisibility};

/// The chief's service.
pub const SERVICE: &str = "pcx.chief.v1.Chief";

/// Each RPC of the service, its tool name, and whether only the view
/// calls it.
const RPCS: [(&str, &str, bool, Effect); 5] = [
    ("Ask", "chief_ask", false, Effect::External),
    ("GetRights", "chief_rights", true, Effect::Read),
    ("ListTalks", "chief_talks", true, Effect::Read),
    ("ListActions", "chief_actions", true, Effect::Read),
    ("GetAction", "chief_action", true, Effect::Read),
];

/// The gRPC path of `method` of the service.
pub fn path(method: &str) -> String {
    format!("/{SERVICE}/{method}")
}

/// Every RPC of the module, as its manifest names it.
pub fn rpcs() -> Vec<Rpc> {
    RPCS.iter()
        .map(|(method, tool, view_only, effect)| Rpc {
            method: path(method),
            tool: (*tool).to_string(),
            visibility: if *view_only {
                vec![ToolVisibility::App.into()]
            } else {
                Vec::new()
            },
            effect: (*effect).into(),
        })
        .collect()
}
