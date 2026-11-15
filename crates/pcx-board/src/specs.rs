// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The board's RPCs as tools (W23): changing a board, and approving,
//! declining, revoking and listing the decisions to approve, are a
//! person's only.

use post_cortex_proto::module::{Effect, Rpc, ToolVisibility};

/// The board's service.
pub const SERVICE: &str = "pcx.board.v1.Boards";

/// Each RPC of the service, its tool name, and whether only a person
/// calls it.
const RPCS: [(&str, &str, bool, Effect); 15] = [
    ("CreateBoard", "board_create", false, Effect::Write),
    ("UpdateBoard", "board_update", true, Effect::Write),
    ("ListBoards", "board_list", false, Effect::Read),
    ("AddTask", "task_add", false, Effect::Write),
    ("UpdateTask", "task_update", false, Effect::Write),
    ("MoveTask", "task_move", false, Effect::Write),
    ("ListTasks", "task_list", false, Effect::Read),
    ("GetTask", "task_get", false, Effect::Read),
    ("AddComment", "comment", false, Effect::Write),
    ("Decide", "decide", false, Effect::Write),
    ("ApproveDecision", "approve_decision", true, Effect::Write),
    ("RevokeApproval", "revoke_approval", true, Effect::Write),
    ("DeclineDecision", "decline_decision", true, Effect::Write),
    (
        "WithdrawDecision",
        "withdraw_decision",
        false,
        Effect::Write,
    ),
    ("ListDecisions", "decision_list", true, Effect::Read),
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
