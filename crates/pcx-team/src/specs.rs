// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The team's RPCs as tools (W23). Who is a member, with what authority,
//! and the trust a team has are a person's to say, so no agent raises its
//! own: those RPCs are for a person only.

use post_cortex_proto::module::{Effect, Rpc, ToolVisibility};

/// The team's service.
pub const SERVICE: &str = "pcx.team.v1.Teams";

/// Each RPC of the service, its tool name, and whether only a person
/// calls it.
const RPCS: [(&str, &str, bool, Effect); 11] = [
    ("GetAutonomy", "autonomy_get", false, Effect::Read),
    ("SetAutonomy", "autonomy_set", true, Effect::Write),
    ("PauseAutonomy", "autonomy_pause", true, Effect::Write),
    ("CreateTeam", "team_create", true, Effect::Write),
    ("ListTeams", "team_list", false, Effect::Read),
    ("AddAgent", "agent_add", true, Effect::Write),
    ("UpdateAgent", "agent_update", true, Effect::Write),
    ("AddMember", "member_add", true, Effect::Write),
    ("RemoveMember", "member_remove", true, Effect::Write),
    ("ListWork", "work_list", false, Effect::Read),
    ("StopWork", "work_stop", true, Effect::Write),
];

/// The gRPC path of `method` of the service.
pub fn path(method: &str) -> String {
    format!("/{SERVICE}/{method}")
}

/// Every RPC of the module, as its manifest names it.
pub fn rpcs() -> Vec<Rpc> {
    RPCS.iter()
        .map(|(method, tool, person_only, effect)| Rpc {
            method: path(method),
            tool: (*tool).to_string(),
            visibility: if *person_only {
                vec![ToolVisibility::App.into()]
            } else {
                Vec::new()
            },
            effect: (*effect).into(),
        })
        .collect()
}
