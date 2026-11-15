// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A call as the operation table sees it: its operation, the sessions and
//! workspaces it names, the project key it resolves, whether it asks for
//! the reranker, and what it counts, read from its JSON form (an MCP call's
//! arguments; a gRPC message or a REST request read into JSON by the
//! transport, `post_cortex_transport::classify`). Beside the table, so the
//! memory instance holds a call to its reach (`crate::reach`) as a gateway
//! reads it.

use serde_json::Value;
use uuid::Uuid;

use crate::tools::operations::{self, Counter, Operation};

mod json;

pub use json::{answer_count, answer_flag, of_json, without_rerank};

/// A request, classified.
#[derive(Debug, Clone)]
pub struct Classified {
    /// Its operation.
    pub operation: &'static Operation,
    /// The sessions it names.
    pub sessions: Vec<Uuid>,
    /// The workspaces it names.
    pub workspaces: Vec<Uuid>,
    /// The project key it resolves or looks up.
    pub project: Option<ProjectKey>,
    /// It asks for the reranker.
    pub rerank: bool,
    /// It writes nothing (an import's preview), whatever the table says.
    pub preview: bool,
    /// What it counts.
    pub counts: Vec<Amount>,
}

/// The operation that makes a new session, in no workspace yet.
const NEW_SESSION: &str = "session.create";

/// The operation that tells the memory instance's health and version.
const HEALTH: &str = "health";

/// The operation that adds a session to a workspace.
const JOIN_WORKSPACE: &str = "workspace.add_session";

impl Classified {
    /// It names no session or workspace: it spans the whole instance.
    #[must_use]
    pub fn instance_wide(&self) -> bool {
        self.sessions.is_empty() && self.workspaces.is_empty()
    }

    /// It reaches no workspace's data, though it names none: making a new
    /// session (which holds nothing yet), or the instance's health. Within
    /// any reach (`crate::reach`).
    #[must_use]
    pub fn reaches_nothing(&self) -> bool {
        self.operation.name == NEW_SESSION || self.operation.name == HEALTH
    }

    /// It changes memory: a write that is no preview, or a delete. An
    /// archived workspace takes none (`crate::archive`).
    #[must_use]
    pub fn changes(&self) -> bool {
        match self.operation.effect {
            operations::Effect::Write { .. } => !self.preview,
            operations::Effect::Delete { .. } => true,
            operations::Effect::Read | operations::Effect::Search { .. } => false,
        }
    }

    /// It adds its sessions to a workspace: within a reach that has the
    /// workspace when each session is new (in no workspace, holding
    /// nothing), which only the memory instance knows (`crate::reach`).
    #[must_use]
    pub fn joins_workspace(&self) -> bool {
        self.operation.name == JOIN_WORKSPACE
    }

    /// It reads or writes the user card: a card call that names no
    /// workspace. For a call for a person it is that person's own card
    /// (`post_cortex_core::core::card::CardScope::user_of`), within any
    /// reach; for one for none it is the host's, every workspace's.
    #[must_use]
    pub fn user_card(&self) -> bool {
        USER_CARD.contains(&self.operation.name) && self.instance_wide()
    }
}

/// The operations of a card, the user card when they name no workspace.
const USER_CARD: [&str; 2] = ["card.get", "card.update"];

/// A project key and the name of the workspace a new key joins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectKey {
    /// The key.
    pub key: String,
    /// The workspace's name (trimmed, never blank).
    pub workspace: Option<String>,
}

/// How much a counter moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Amount {
    /// The counter.
    pub counter: Counter,
    /// By how much.
    pub by: By,
}

/// The size of an [`Amount`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum By {
    /// This many (a delete gives them back: the counter goes down).
    Known(i64),
    /// The value of this field of the answer ([`answer_count`] for a JSON
    /// answer; the transport reads a gRPC answer's the same way).
    Answer(&'static str),
}

/// Why a request could not be classified.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ClassifyError {
    /// No such MCP tool.
    #[error("unknown tool '{0}'")]
    UnknownTool(String),
    /// The tool has no such action (or no routing argument).
    #[error("{tool}: unknown {selector} '{value}'")]
    UnknownAction {
        /// The tool.
        tool: String,
        /// The selector argument.
        selector: String,
        /// The value given.
        value: String,
    },
    /// No such RPC.
    #[error("unknown RPC '{0}'")]
    UnknownRpc(String),
    /// No such REST route.
    #[error("unknown route '{0}'")]
    UnknownRoute(String),
    /// The request does not decode.
    #[error("malformed request: {0}")]
    Malformed(String),
    /// A field that names a session or workspace holds no id.
    #[error("{field}: '{value}' is not an id")]
    BadId {
        /// The field.
        field: String,
        /// Its value.
        value: String,
    },
    /// A count field is not a number.
    #[error("{field}: not a count")]
    BadCount {
        /// The field.
        field: String,
    },
}

/// The MCP call of `tool` with `arguments`.
///
/// # Errors
/// An unknown tool or action, or a field that names no id.
pub fn mcp(tool: &str, arguments: &Value) -> Result<Classified, ClassifyError> {
    if !operations::mcp_tools().contains(&tool) {
        return Err(ClassifyError::UnknownTool(tool.to_string()));
    }
    let operation = match operations::selector(tool) {
        Some(selector) => {
            let value = arguments
                .get(selector)
                .and_then(Value::as_str)
                .unwrap_or_default();
            routed(tool, Some(value)).ok_or_else(|| ClassifyError::UnknownAction {
                tool: tool.to_string(),
                selector: selector.to_string(),
                value: value.to_string(),
            })?
        }
        None => by_argument(tool, arguments)?,
    };
    of_json(operation, arguments)
}

/// The operation of `tool` whose selector value is `value` (any case).
fn routed(tool: &str, value: Option<&str>) -> Option<&'static Operation> {
    let value = value?;
    operations::OPERATIONS.iter().find(|op| {
        op.mcp.iter().any(|route| {
            route.tool == tool && route.values.iter().any(|v| v.eq_ignore_ascii_case(value))
        })
    })
}

/// The operation of a tool without a selector: the last route (in table
/// order) whose routing argument is given (not null, not `false`), else
/// the tool's default route.
fn by_argument(tool: &str, arguments: &Value) -> Result<&'static Operation, ClassifyError> {
    let given = |name: &&str| {
        arguments
            .get(*name)
            .is_some_and(|v| !v.is_null() && *v != Value::Bool(false))
    };
    let routes = || {
        operations::OPERATIONS
            .iter()
            .flat_map(|op| op.mcp.iter().map(move |route| (op, route)))
            .filter(move |(_, route)| route.tool == tool)
    };
    routes()
        .rfind(|(_, route)| route.values.iter().any(given))
        .or_else(|| routes().find(|(_, route)| route.values.is_empty()))
        .map(|(op, _)| op)
        .ok_or_else(|| ClassifyError::UnknownAction {
            tool: tool.to_string(),
            selector: "arguments".to_string(),
            value: String::new(),
        })
}
