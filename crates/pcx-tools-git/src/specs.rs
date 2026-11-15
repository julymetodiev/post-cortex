// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The RPCs of `tools-git` as tools (W23), for agents and people alike.

use post_cortex_proto::module::{Effect, Rpc};

/// The module's service.
pub const SERVICE: &str = "pcx.tools.git.v1.Git";

/// Each RPC of the service and its tool name.
const RPCS: [(&str, &str, Effect); 10] = [
    ("Status", "git_status", Effect::Read),
    ("Diff", "git_diff", Effect::Read),
    ("Log", "git_log", Effect::Read),
    ("Branches", "git_branches", Effect::Read),
    ("CreateBranch", "git_branch", Effect::Write),
    ("Switch", "git_switch", Effect::Write),
    ("Commit", "git_commit", Effect::Write),
    ("BranchCommits", "git_branch_commits", Effect::Read),
    ("AddWorktree", "git_worktree_add", Effect::Write),
    // It changes a remote outside pcx.
    ("Push", "git_push", Effect::External),
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
