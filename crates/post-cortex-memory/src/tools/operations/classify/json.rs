// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The table's fields read from the JSON form of a request: the shared
//! core of every surface.

use crate::content_vectorizer::RerankSkipped;
use crate::tools::operations::{Count, Effect, Operation, Tally};
use serde_json::Value;
use uuid::Uuid;

use crate::tools::operations::classify::{Amount, By, Classified, ClassifyError, ProjectKey};

/// The value at `path` (`settings.commit`) of `json`.
fn at<'a>(json: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.')
        .try_fold(json, |value, part| value.get(part))
        .filter(|v| !v.is_null())
}

/// The ids at `path`: a string or an array of strings; empty strings are
/// absent.
fn ids(json: &Value, path: &str) -> Result<Vec<Uuid>, ClassifyError> {
    let parse = |raw: &str| {
        let raw = raw.trim();
        Uuid::parse_str(raw).map_err(|_| ClassifyError::BadId {
            field: path.to_string(),
            value: raw.to_string(),
        })
    };
    match at(json, path) {
        None => Ok(Vec::new()),
        Some(Value::String(s)) if s.trim().is_empty() => Ok(Vec::new()),
        Some(Value::String(s)) => Ok(vec![parse(s)?]),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| match item {
                Value::String(s) => parse(s),
                other => Err(ClassifyError::BadId {
                    field: path.to_string(),
                    value: other.to_string(),
                }),
            })
            .collect(),
        Some(other) => Err(ClassifyError::BadId {
            field: path.to_string(),
            value: other.to_string(),
        }),
    }
}

/// `json` with a `scope` of `session` or `workspace` written as the field
/// it stands for: its id is in `scope_id` (MCP) or `id` (REST).
fn with_scope_selector(json: &Value) -> Value {
    let mut json = json.clone();
    let Some(object) = json.as_object_mut() else {
        return json;
    };
    let field = match object.get("scope").and_then(Value::as_str) {
        Some(s) if s.eq_ignore_ascii_case("session") => "session_id",
        Some(s) if s.eq_ignore_ascii_case("workspace") => "workspace_id",
        _ => return json,
    };
    if let Some(id) = object.get("scope_id").or_else(|| object.get("id")).cloned() {
        object.insert(field.to_string(), id);
    }
    json
}

/// The project key `json` resolves (`key`, `workspace` and `name`).
fn project(json: &Value) -> Option<ProjectKey> {
    let key = at(json, "key")?.as_str()?.to_string();
    let text = |field: &str| {
        at(json, field)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(String::from)
    };
    Some(ProjectKey {
        key,
        workspace: text("workspace"),
        name: text("name"),
    })
}

fn amounts(tallies: &[Tally], json: &Value, sign: i64) -> Result<Vec<Amount>, ClassifyError> {
    tallies
        .iter()
        .map(|tally| {
            let by = match tally.by {
                Count::One => By::Known(sign),
                Count::Items(field) => {
                    let bad = || ClassifyError::BadCount {
                        field: field.to_string(),
                    };
                    let n = match at(json, field) {
                        None => 0,
                        Some(Value::Array(items)) => items.len(),
                        Some(_) => return Err(bad()),
                    };
                    By::Known(sign * i64::try_from(n).map_err(|_| bad())?)
                }
                Count::Answer(field) => By::Answer(field),
            };
            Ok(Amount {
                counter: tally.counter,
                by,
            })
        })
        .collect()
}

/// `operation` with the request `json`.
pub fn of_json(operation: &'static Operation, json: &Value) -> Result<Classified, ClassifyError> {
    let projects = matches!(operation.name, "project.resolve" | "project.find");
    let json = with_scope_selector(json);
    let mut sessions = Vec::new();
    for field in operation.scope.sessions {
        sessions.extend(ids(&json, field)?);
    }
    let mut workspaces = Vec::new();
    // A project's `workspace` is a name, not an id: it is the project key's.
    if !projects {
        for field in operation.scope.workspaces {
            workspaces.extend(ids(&json, field)?);
        }
    }
    let (rerank, preview, counts) = match operation.effect {
        Effect::Read => (false, false, Vec::new()),
        Effect::Search { rerank } => {
            let asks = rerank.is_some_and(|r| {
                at(&json, r.field)
                    .and_then(Value::as_str)
                    .is_some_and(|v| r.values.iter().any(|x| x.eq_ignore_ascii_case(v)))
            });
            (asks, false, Vec::new())
        }
        Effect::Write { counts, commit } => {
            let preview = commit.is_some_and(|field| at(&json, field) != Some(&Value::Bool(true)));
            let counts = if preview {
                Vec::new()
            } else {
                amounts(counts, &json, 1)?
            };
            (false, preview, counts)
        }
        Effect::Delete { counts } => (false, false, amounts(counts, &json, -1)?),
    };
    Ok(Classified {
        operation,
        sessions,
        workspaces,
        project: projects.then(|| project(&json)).flatten(),
        rerank,
        preview,
        counts,
    })
}

/// The count in `field` of a JSON answer.
///
/// # Errors
/// The field is missing or not a whole number.
pub fn answer_count(answer: &Value, field: &str) -> Result<i64, ClassifyError> {
    let bad = || ClassifyError::BadCount {
        field: field.to_string(),
    };
    let value = at(answer, field).ok_or_else(bad)?;
    match value {
        Value::Number(n) => n.as_i64().ok_or_else(bad),
        // proto3 JSON writes 64-bit integers as strings.
        Value::String(s) => s.parse().map_err(|_| bad()),
        _ => Err(bad()),
    }
}

/// The flag in `field` of a JSON answer; `None` when the answer does not
/// carry it (a selection left it out) or it is not a boolean.
#[must_use]
pub fn answer_flag(answer: &Value, field: &str) -> Option<bool> {
    at(answer, field).and_then(Value::as_bool)
}

/// The JSON request of `operation` without its reranker: its rerank field
/// set to the fallback, and marked as withheld by the plan (its answer says
/// so). Nothing changes for an operation that never
/// reranks.
pub fn without_rerank(operation: &Operation, request: &mut Value) {
    let Effect::Search {
        rerank: Some(rerank),
    } = operation.effect
    else {
        return;
    };
    if let Some(object) = request.as_object_mut() {
        object.insert(rerank.field.to_string(), Value::from(rerank.fallback));
        object.insert(
            rerank.withheld.to_string(),
            Value::from(RerankSkipped::PlanLimit.as_str()),
        );
    }
}
