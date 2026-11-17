// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The operation table (ARCHITECTURE section 6): the single list of
//! user-facing operations, the capability method of each, and the MCP
//! tool and action, gRPC RPC and REST route that expose it.
//!
//! It is data: the MCP server takes each tool's valid actions from it, the
//! tools reference is rendered from it, and the parity test fails when a
//! transport exposes something the table does not list or lacks what it
//! lists. Each row also states its effect and scope fields (`effect`).

pub mod classify;
mod effect;
mod table;

pub use effect::{Count, Counter, Effect, RERANKED, Rerank, ScopeFields, Tally};
pub use table::OPERATIONS;

/// One user-facing operation.
#[derive(Debug, Clone, Copy)]
pub struct Operation {
    /// Its name (`session.create`).
    pub name: &'static str,
    /// The capability method that implements it (`sessions::create`).
    pub capability: &'static str,
    /// What it does, in one line.
    pub summary: &'static str,
    /// The serde response shape every transport carries as JSON
    /// (`SearchResponse`), or `text` for a reply without data.
    pub response: &'static str,
    /// The MCP routes to it.
    pub mcp: &'static [McpRoute],
    /// The gRPC RPCs of `pcx.v1.PostCortex` that expose it.
    pub grpc: &'static [&'static str],
    /// The gRPC RPCs of `pcx.views.v1.Views` that expose it.
    pub views: &'static [&'static str],
    /// The gRPC RPCs of `pcx.v1.Blobs` that expose it (W24).
    pub blobs: &'static [&'static str],
    /// The REST routes (`POST /api/search`) that expose it.
    pub rest: &'static [&'static str],
    /// What it does to memory.
    pub effect: Effect,
    /// The fields that name the sessions and workspaces it touches.
    pub scope: ScopeFields,
}

/// How an MCP tool call reaches an operation: the tool, and the values of
/// its selector argument ([`selector`]) that route to it. A tool without a
/// selector (`update_conversation_context`) lists the argument whose
/// presence routes the call (`updates`, `dry_run`). No values: every call
/// of the tool that no other route of the tool takes.
#[derive(Debug, Clone, Copy)]
pub struct McpRoute {
    /// The tool.
    pub tool: &'static str,
    /// The selector values (`action`, `query_type`), or routing arguments.
    pub values: &'static [&'static str],
}

/// The argument of a tool that selects the operation.
#[must_use]
pub fn selector(tool: &str) -> Option<&'static str> {
    match tool {
        "session" | "manage_workspace" | "manage_document" | "manage_messages"
        | "manage_entity" | "admin" => Some("action"),
        "query_conversation_context" => Some("query_type"),
        _ => None,
    }
}

/// The operation named `name`.
#[must_use]
pub fn operation(name: &str) -> Option<&'static Operation> {
    OPERATIONS.iter().find(|op| op.name == name)
}

/// Every MCP tool of the table, in table order, once.
#[must_use]
pub fn mcp_tools() -> Vec<&'static str> {
    let mut tools: Vec<&'static str> = Vec::new();
    for route in OPERATIONS.iter().flat_map(|op| op.mcp) {
        if !tools.contains(&route.tool) {
            tools.push(route.tool);
        }
    }
    tools
}

/// The selector values of `tool` that the table routes, in table order.
#[must_use]
pub fn mcp_values(tool: &str) -> Vec<&'static str> {
    OPERATIONS
        .iter()
        .flat_map(|op| op.mcp)
        .filter(|route| route.tool == tool)
        .flat_map(|route| route.values.iter().copied())
        .collect()
}

/// The operation an MCP call of `tool` with selector `value` reaches.
#[must_use]
pub fn mcp_operation(tool: &str, value: Option<&str>) -> Option<&'static Operation> {
    let routes = || {
        OPERATIONS
            .iter()
            .flat_map(|op| op.mcp.iter().map(move |r| (op, r)))
            .filter(move |(_, r)| r.tool == tool)
    };
    let by_value = value.and_then(|v| {
        routes()
            .find(|(_, r)| r.values.iter().any(|x| x.eq_ignore_ascii_case(v)))
            .map(|(op, _)| op)
    });
    by_value.or_else(|| {
        routes()
            .find(|(_, r)| r.values.is_empty())
            .map(|(op, _)| op)
    })
}

/// Every gRPC RPC of the table.
#[must_use]
pub fn grpc_methods() -> Vec<&'static str> {
    OPERATIONS.iter().flat_map(|op| op.grpc).copied().collect()
}

/// Every `pcx.views.v1.Views` RPC of the table.
#[must_use]
pub fn views_methods() -> Vec<&'static str> {
    OPERATIONS.iter().flat_map(|op| op.views).copied().collect()
}

/// Every `pcx.v1.Blobs` RPC of the table.
#[must_use]
pub fn blobs_methods() -> Vec<&'static str> {
    OPERATIONS.iter().flat_map(|op| op.blobs).copied().collect()
}

/// Every REST route of the table (`METHOD /path`).
#[must_use]
pub fn rest_routes() -> Vec<&'static str> {
    OPERATIONS.iter().flat_map(|op| op.rest).copied().collect()
}

#[cfg(test)]
mod tests;
