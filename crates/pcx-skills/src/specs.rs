// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The skills' RPCs as tools (W23), for agents and people alike.

use post_cortex_proto::module::{Effect, Rpc};

/// The skills' service.
pub const SERVICE: &str = "pcx.skills.v1.Skills";

/// Each RPC of the service and its tool name.
const RPCS: [(&str, &str, Effect); 4] = [
    ("AddSkill", "skill_add", Effect::Write),
    ("UpdateSkill", "skill_update", Effect::Write),
    ("ListSkills", "skill_list", Effect::Read),
    ("GetSkill", "skill_get", Effect::Read),
];

/// The gRPC path of `method` of the service.
pub fn path(method: &str) -> String {
    format!("/{SERVICE}/{method}")
}

/// Every RPC of the module, as its manifest names it.
pub fn rpcs() -> Vec<Rpc> {
    RPCS.iter()
        .map(|(method, tool, effect)| Rpc {
            method: path(method),
            tool: (*tool).to_string(),
            visibility: Vec::new(),
            effect: (*effect).into(),
        })
        .collect()
}
