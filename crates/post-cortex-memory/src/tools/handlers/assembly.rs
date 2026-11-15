// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! Tool `assemble_context`: graph-aware context over a session or a
//! workspace (the Assembly capability), with an optional JSONPath `select`.

use crate::Memory;
use crate::capabilities::{AssembleRequest, AssemblyError, DEFAULT_TOKEN_BUDGET, Target};
use serde_json::Value;
use uuid::Uuid;

use crate::tools::args::AssembleContextRequest;
use crate::tools::check;
use crate::tools::handlers::Outcome;
use crate::tools::parse::{FieldHint, arguments};
use crate::tools::reply;
use crate::tools::responses::Assembled;
use crate::tools::select::Select;
use crate::tools::{Failure, wire};

const HINTS: &[FieldHint] = &[FieldHint {
    field: "query",
    expected: "non-empty query/topic string",
    hint: "Provide the question or topic to assemble context for",
}];

pub async fn handle(memory: &Memory, params: Value, person: Option<Uuid>) -> Outcome {
    let req: AssembleContextRequest = arguments(params, HINTS)?;
    // Empty ids are missing ids; the scope is resolved once from what is left.
    let id = |raw: &Option<String>| {
        raw.as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let token_budget = req
        .token_budget
        .filter(|b| *b > 0)
        .map_or(DEFAULT_TOKEN_BUDGET, |b| b as usize);
    let corpus = wire::sources(req.sources.as_deref())?;
    let target = match (id(&req.workspace_id), id(&req.session_id)) {
        (Some(raw), _) => Target::Workspace(Uuid::parse_str(&raw).map_err(|_| {
            check::invalid(
                "Invalid UUID: Invalid UUID format",
                "workspace_id",
                check::Problem::BadUuid,
                Some(Value::String(raw.clone())),
            )
            .expected("Valid UUID string")
        })?),
        (None, Some(raw)) => Target::Session(check::session_id(&raw)?),
        (None, None) => {
            return Err(Failure::invalid(
                "either session_id or workspace_id is required",
            ));
        }
    };
    let context = memory
        .assembly
        .assemble(AssembleRequest {
            query: req.query.clone(),
            target,
            token_budget,
            corpus,
            person,
        })
        .await
        .map_err(|e| match e {
            // The tool has always answered with the bare reason.
            AssemblyError::InvalidArgument(msg) => Failure::invalid(msg).parameter("query"),
            e => Failure::from(e),
        })?;
    let message = format!(
        "Assembled {} items ({} tokens) for query: {}",
        context.items.len(),
        context.total_tokens,
        req.query
    );
    let response = Assembled::new(req.query, token_budget, context);
    reply::selected(message, &response, &Select::new(req.select, req.select_map))
}

#[cfg(test)]
mod tests;
