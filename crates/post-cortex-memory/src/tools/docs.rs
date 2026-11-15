// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The tools reference (`docs/TOOLS.md`), rendered by a test from the
//! operation table and the tool list, so the documentation cannot drift
//! from what the transports expose.

use crate::tools::catalog::ToolCatalog;
use crate::tools::operations::{Count, Counter, Effect, OPERATIONS, Operation, Tally, selector};

/// The tools reference as Markdown.
#[must_use]
pub fn tools_reference(catalog: &ToolCatalog) -> String {
    let mut out = String::from(
        "# Tools reference\n\n\
         Generated from the operation table (`post-cortex-memory`, \
         `tools::operations::OPERATIONS`) by a test of `post-cortex-memory`; \
         do not edit by hand. A test fails when this file and the table differ.\n\n\
         ## Operations\n\n\
         | Operation | Capability method | MCP | gRPC | REST | Response |\n\
         |---|---|---|---|---|---|\n",
    );
    for op in OPERATIONS {
        out.push_str(&format!(
            "| `{}` | `{}` | {} | {} | {} | `{}` |\n",
            op.name,
            op.capability,
            cell(&mcp_cell(op)),
            cell(
                &op.grpc
                    .iter()
                    .map(|rpc| (*rpc).to_string())
                    .chain(op.views.iter().map(|rpc| format!("Views/{rpc}")))
                    .chain(op.blobs.iter().map(|rpc| format!("Blobs/{rpc}")))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            cell(
                &op.rest
                    .iter()
                    .map(|r| format!("`{r}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            op.response,
        ));
    }
    out.push_str(
        "\n## Effects\n\n\
         What each operation does to memory, and the request fields that name \
         its sessions and workspaces (none: the whole instance). A gateway \
         admits, limits and counts calls from these.\n\n\
         | Operation | Effect | Sessions | Workspaces |\n\
         |---|---|---|---|\n",
    );
    for op in OPERATIONS {
        let fields = |names: &[&str]| {
            names
                .iter()
                .map(|n| format!("`{n}`"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        out.push_str(&format!(
            "| `{}` | {} | {} | {} |\n",
            op.name,
            effect_cell(&op.effect),
            cell(&fields(op.scope.sessions)),
            cell(&fields(op.scope.workspaces)),
        ));
    }
    out.push_str("\n## MCP tools\n");
    for tool in catalog.tools() {
        out.push_str(&format!("\n### `{}`\n\n", tool.name));
        out.push_str(&tool.description);
        out.push_str("\n\n");
        let ops: Vec<String> = OPERATIONS
            .iter()
            .filter(|op| op.mcp.iter().any(|r| r.tool == tool.name))
            .map(|op| format!("`{}` ({})", op.name, op.summary))
            .collect();
        out.push_str(&format!("Operations: {}.\n\n", ops.join("; ")));
        let mut args: Vec<(String, bool)> = tool
            .input_schema
            .get("properties")
            .and_then(|p| p.as_object())
            .map(|p| p.keys().map(|k| (k.clone(), false)).collect())
            .unwrap_or_default();
        let required: Vec<&str> = tool
            .input_schema
            .get("required")
            .and_then(|r| r.as_array())
            .map(|r| r.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default();
        for (name, req) in &mut args {
            *req = required.contains(&name.as_str());
        }
        let args: Vec<String> = args
            .iter()
            .map(|(name, req)| {
                if *req {
                    format!("`{name}` (required)")
                } else {
                    format!("`{name}`")
                }
            })
            .collect();
        out.push_str(&format!("Arguments: {}.\n", args.join(", ")));
    }
    out
}

/// The MCP column of an operation: `tool action|action`.
fn mcp_cell(op: &Operation) -> String {
    op.mcp
        .iter()
        .map(|r| match (selector(r.tool), r.values.is_empty()) {
            (_, true) => format!("`{}`", r.tool),
            (Some(arg), false) => format!("`{}` {arg}={}", r.tool, r.values.join("\\|")),
            (None, false) => format!("`{}` with `{}`", r.tool, r.values.join("`, `")),
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// `+records 1`, `-messages answer.deleted`, ...
fn tallies(sign: char, counts: &[Tally]) -> String {
    counts
        .iter()
        .map(|t| {
            let counter = match t.counter {
                Counter::Records => "records",
                Counter::Messages => "messages",
                Counter::Workspaces => "workspaces",
            };
            let by = match t.by {
                Count::One => "1".to_string(),
                Count::Items(field) => format!("per `{field}`"),
                Count::Answer(field) => format!("answer `{field}`"),
            };
            format!(" ({sign}{counter} {by})")
        })
        .collect()
}

/// The effect column of an operation.
fn effect_cell(effect: &Effect) -> String {
    match effect {
        Effect::Read => "read".to_string(),
        Effect::Search { rerank: None } => "search".to_string(),
        Effect::Search {
            rerank: Some(rerank),
        } => format!(
            "search, reranked when `{}` is {}",
            rerank.field,
            rerank.values.join("\\|")
        ),
        Effect::Write {
            counts,
            commit: None,
        } => format!("write{}", tallies('+', counts)),
        Effect::Write {
            counts,
            commit: Some(flag),
        } => format!("write when `{flag}`, else read{}", tallies('+', counts)),
        Effect::Delete { counts } => format!("delete{}", tallies('-', counts)),
    }
}

fn cell(s: &str) -> String {
    if s.is_empty() {
        "-".to_string()
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests;
