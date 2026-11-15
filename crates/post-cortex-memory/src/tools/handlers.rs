// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! The tool handlers: arguments into the typed capability request, the
//! capability call, the result or failure back. One module per tool.

use pcx_module_api::ToolOutput;
use post_cortex_ports::DocumentKinds;
use serde_json::Value;

use crate::Memory;
use crate::tools::Failure;
use crate::tools::operations;

mod admin;
mod assembly;
mod document;
mod entity;
#[cfg(test)]
mod lifecycle_tests;
mod messages;
mod query;
mod search;
mod session;
mod summary;
mod update_context;
mod workspace;

/// What a tool handler returns.
pub type Outcome = Result<ToolOutput, Failure>;

/// Who calls a tool: the default author of its writes, and the module the
/// host names as the caller with the port to the running modules' document
/// kinds.
pub struct Caller<'a> {
    /// The author of its writes unless they name one.
    pub author: Option<String>,
    /// The calling module (`Scope.caller_module`); none: not a module.
    pub module: Option<&'a str>,
    /// The document kinds of the running modules.
    pub kinds: &'a dyn DocumentKinds,
    /// The reach of the call (docs/connect.md 4a), for its writes.
    pub reach: Option<Vec<uuid::Uuid>>,
    /// The person the call is for (`Scope.person`), whose user card it reads
    /// and writes; none: the host's.
    pub person: Option<uuid::Uuid>,
}

/// Run `tool`; `None` for a tool that does not exist.
pub async fn dispatch(
    memory: &Memory,
    tool: &str,
    arguments: Value,
    caller: &Caller<'_>,
) -> Option<Outcome> {
    let author = caller.author.clone();
    Some(match tool {
        "session" => session::handle(memory, arguments, author).await,
        "update_conversation_context" => update_context::handle(memory, arguments, author).await,
        "semantic_search" => search::handle(memory, arguments).await,
        "get_structured_summary" => summary::handle(memory, arguments).await,
        "query_conversation_context" => query::handle(memory, arguments, caller.person).await,
        "manage_workspace" => workspace::handle(memory, arguments, author, caller.person).await,
        "manage_document" => document::handle(memory, arguments, caller).await,
        "manage_messages" => messages::handle(memory, arguments, author).await,
        "assemble_context" => assembly::handle(memory, arguments, caller.person).await,
        "manage_entity" => entity::handle(memory, arguments).await,
        "admin" => admin::handle(memory, arguments).await,
        _ => return None,
    })
}

/// The table's actions of `tool`, lowercased `action` checked against them.
fn action(tool: &str, action: &str, hint: &str) -> Result<String, Failure> {
    crate::tools::check::action(action, &operations::mcp_values(tool), hint)?;
    Ok(action.to_lowercase())
}

/// The `Invalid action` error of an action the table routes but the tool
/// does not handle (a table/handler mismatch the parity test catches).
fn unhandled(tool: &str, action: &str) -> Failure {
    Failure::invalid(format!(
        "Invalid action '{action}'. Use: {}",
        operations::mcp_values(tool).join(" | ")
    ))
    .parameter("action")
}

/// The writer's own name for itself, else `default` (the caller's).
pub fn author(explicit: Option<&str>, default: Option<String>) -> Option<String> {
    explicit
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .map(String::from)
        .or(default)
}
