// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! Tool `query_conversation_context`: one tool for many operations, chosen
//! by `query_type` through the operation table.

use std::collections::HashMap;

use crate::Memory;
use serde_json::Value;
use uuid::Uuid;

use crate::tools::args::QueryConversationContextRequest;
use crate::tools::check;
use crate::tools::handlers::Outcome;
use crate::tools::parse::{FieldHint, arguments};
use crate::tools::{Failure, operations};

mod views;

const TOOL: &str = "query_conversation_context";

const HINTS: &[FieldHint] = &[
    FieldHint {
        field: "session_id",
        expected: "UUID string (36 chars with hyphens)",
        hint: "Provide the session UUID you want to query",
    },
    FieldHint {
        field: "query_type",
        expected: "query type string (e.g., entity_importance, entity_network, find_related_entities)",
        hint: "Specify the type of query: entity_importance, entity_network, find_related_entities, get_entity_context, search_updates",
    },
    FieldHint {
        field: "parameters",
        expected: "object with string key-value pairs",
        hint: "Query parameters vary by query_type. Provide as key-value pairs.",
    },
];

pub async fn handle(memory: &Memory, params: Value, person: Option<uuid::Uuid>) -> Outcome {
    let req: QueryConversationContextRequest = arguments(params, HINTS)?;
    let session_id = check::session_id(&req.session_id)?;
    let query_type = req.query_type.to_lowercase();
    let Some(operation) = operations::mcp_operation(TOOL, Some(&query_type)) else {
        return Err(
            Failure::invalid(format!("Unknown query type: {}", req.query_type))
                .parameter("query_type"),
        );
    };
    let p = &req.parameters;
    match operation.name {
        "view" => views::view(memory, &query_type, session_id, p).await,
        "query" => views::query(memory, &query_type, session_id, p).await,
        "assemble" => views::assemble(memory, session_id, p, person).await,
        "search.related" => views::related(memory, session_id, p).await,
        "summary.decisions" => views::key_decisions(memory, session_id).await,
        "summary.insights" => views::key_insights(memory, session_id, number(p, "limit")?).await,
        "entity.importance" => {
            let limit = number(p, "limit")?;
            let min = number(p, "min_importance")?;
            views::importance(memory, session_id, limit, min).await
        }
        "entity.network" => {
            let center = p.get("center_entity").map(String::as_str);
            let entities = number(p, "max_entities")?;
            let relationships = number(p, "max_relationships")?;
            views::network(memory, session_id, center, entities, relationships).await
        }
        "session.statistics" => views::statistics(memory, session_id).await,
        "views.record" => views::record(memory, session_id, p).await,
        other => Err(Failure::internal(format!(
            "query type {query_type} routes to {other}, which the tool does not handle"
        ))),
    }
}

/// Parameter `name` parsed strictly: absent or blank is `None`; a value
/// that does not parse is an invalid argument naming it, never a default.
fn param<T: std::str::FromStr>(
    parameters: &HashMap<String, String>,
    name: &str,
    expected: &str,
) -> Result<Option<T>, Failure> {
    match parameters.get(name).map(|v| v.trim()) {
        None | Some("") => Ok(None),
        Some(value) => value.parse().map(Some).map_err(|_| {
            Failure::invalid(format!("parameter {name:?}: {value:?} is not {expected}"))
                .parameter(name)
                .received(value)
        }),
    }
}

/// A numeric parameter (see [`param`]).
fn number<T: std::str::FromStr>(
    parameters: &HashMap<String, String>,
    name: &str,
) -> Result<Option<T>, Failure> {
    param(parameters, name, "a number")
}

/// A session or the workspace of its `workspace_id` parameter.
fn workspace_param(parameters: &HashMap<String, String>) -> Result<Option<Uuid>, Failure> {
    parameters
        .get("workspace_id")
        .filter(|v| !v.is_empty())
        .map(|raw| {
            Uuid::parse_str(raw).map_err(|_| {
                Failure::invalid(format!("workspace_id: invalid UUID '{raw}'"))
                    .parameter("workspace_id")
            })
        })
        .transpose()
}
