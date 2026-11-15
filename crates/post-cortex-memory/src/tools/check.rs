// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! Checks of tool arguments beyond their types (UUIDs, closed value sets,
//! limits), and the recovery suggestions of a failed argument, chosen by
//! the argument and the kind of problem.

use serde_json::Value;
use uuid::Uuid;

use crate::tools::Failure;

/// What is wrong with an argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Problem {
    /// A required argument is absent.
    Missing,
    /// It has the wrong JSON type.
    WrongType,
    /// It is not a UUID.
    BadUuid,
    /// It is not one of a closed set of values.
    NotOneOf,
    /// It is out of range.
    OutOfRange,
    /// The session it names does not exist.
    NotFound,
}

/// Valid interaction type values accepted by the write tool.
pub const VALID_INTERACTION_TYPES: &[&str] = &[
    "qa",
    "decision_made",
    "problem_solved",
    "code_change",
    "requirement_added",
    "concept_defined",
];

const UUID_SHAPE: &str =
    "UUID format: xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx (36 chars with hyphens)";

/// An invalid argument with its suggestions.
pub fn invalid(
    message: impl Into<String>,
    field: &str,
    problem: Problem,
    received: Option<Value>,
) -> Failure {
    let suggestions = suggestions(field, problem, received.as_ref());
    let mut failure = Failure::invalid(message)
        .parameter(field)
        .suggestions(suggestions);
    if let Some(received) = received {
        failure = failure.received(received);
    }
    failure
}

/// A required argument that is absent.
pub fn missing(field: &str, expected: &str, hint: &str) -> Failure {
    invalid(
        format!("Missing required parameter: {field} required"),
        field,
        Problem::Missing,
        None,
    )
    .expected(expected)
    .hint(hint)
}

/// A session id argument.
pub fn session_id(raw: &str) -> Result<Uuid, Failure> {
    uuid(
        "session_id",
        raw,
        &format!("Invalid UUID format: '{raw}': Invalid UUID"),
        UUID_SHAPE,
        "Create a session first using the 'session' tool with action='create', or search for existing sessions using 'semantic_search'",
    )
}

/// A workspace id argument.
pub fn workspace_id(raw: &str) -> Result<Uuid, Failure> {
    uuid(
        "workspace_id",
        raw,
        &format!("Invalid workspace ID format: '{raw}': Invalid UUID"),
        UUID_SHAPE,
        "Use the 'manage_workspace' tool with action='list' to see available workspaces, or create one with action='create'",
    )
}

/// A UUID argument named `field`.
pub fn uuid(
    field: &str,
    raw: &str,
    message: &str,
    expected: &str,
    hint: &str,
) -> Result<Uuid, Failure> {
    Uuid::parse_str(raw).map_err(|_| {
        invalid(message, field, Problem::BadUuid, Some(raw.into()))
            .expected(expected)
            .hint(hint)
    })
}

/// A value of a closed set (lowercase); `what` names it in the message.
fn one_of(field: &str, what: &str, value: &str, valid: &[&str], hint: &str) -> Result<(), Failure> {
    if valid.contains(&value.to_lowercase().as_str()) {
        return Ok(());
    }
    Err(invalid(
        format!("Invalid {what}: '{value}': Invalid {what}"),
        field,
        Problem::NotOneOf,
        Some(value.into()),
    )
    .expected(format!("one of: {}", valid.join(", ")))
    .hint(hint))
}

/// The interaction type of a single write.
pub fn interaction_type(value: &str) -> Result<(), Failure> {
    if VALID_INTERACTION_TYPES.contains(&value.to_lowercase().as_str()) {
        return Ok(());
    }
    Err(invalid(
        format!("Invalid interaction_type: '{value}': Invalid type"),
        "interaction_type",
        Problem::NotOneOf,
        Some(value.into()),
    )
    .expected(format!("one of: {}", VALID_INTERACTION_TYPES.join(", ")))
    .hint("Use exact lowercase term with underscores. Valid types: qa, decision_made, problem_solved, code_change, requirement_added, concept_defined"))
}

/// The scope of a search.
pub fn scope(value: &str) -> Result<(), Failure> {
    crate::tools::wire::scope(value).map(drop).map_err(|f| {
        let received = f.details.received.clone();
        f.hint("Valid scopes: 'session' (requires scope_id), 'workspace' (requires scope_id), 'global' (default, no scope_id needed)")
            .suggestions(suggestions("scope", Problem::NotOneOf, received.as_ref()))
    })
}

/// The action of a tool: one of `valid` (the operation table's actions of
/// the tool).
pub fn action(value: &str, valid: &[&str], hint: &str) -> Result<(), Failure> {
    one_of("action", "action", value, valid, hint)
}

/// A session's role in a workspace.
pub fn role(value: &str) -> Result<(), Failure> {
    one_of(
        "role",
        "role",
        value,
        &["primary", "related", "dependency", "shared"],
        "Valid session roles: primary (main session), related (related context), dependency (required context), shared (shared context)",
    )
}

/// A recency bias: finite, 0.0 to 10.0.
pub fn recency_bias(value: Option<f32>) -> Result<Option<f32>, Failure> {
    const MAX: f32 = 10.0;
    let Some(v) = value else {
        return Ok(None);
    };
    if !v.is_finite() {
        return Err(invalid(
            "Invalid recency_bias value: NaN or Infinity not allowed",
            "recency_bias",
            Problem::OutOfRange,
            Some(Value::String(v.to_string())),
        )
        .expected("finite f32 between 0.0 and 10.0")
        .hint("Use a finite value between 0.0 (disabled) and 10.0 (aggressive decay). Recommended: 0.0-1.0 for most use cases."));
    }
    if !(0.0..=MAX).contains(&v) {
        return Err(invalid(
            "recency_bias out of range: Value must be between 0.0 and 10.0",
            "recency_bias",
            Problem::OutOfRange,
            Some(Value::String(v.to_string())),
        )
        .expected("f32 in range [0.0, 10.0]")
        .hint(format!(
            "Use recency_bias between 0 and {MAX}, or omit for default (0.0 = disabled)"
        )));
    }
    Ok(Some(v))
}

/// A limit argument `field`: 1 to `max`, `default` when absent.
pub fn limit(
    field: &str,
    value: Option<usize>,
    default: usize,
    max: usize,
) -> Result<usize, Failure> {
    crate::tools::wire::limit(field, value, default, max).map_err(|f| {
        let received = f.details.received.clone();
        f.suggestions(suggestions(field, Problem::OutOfRange, received.as_ref()))
    })
}

/// What usually fixes `problem` with `field` (`received` was given).
pub fn suggestions(field: &str, problem: Problem, received: Option<&Value>) -> Vec<String> {
    let mut out = Vec::new();
    if problem == Problem::BadUuid {
        out.push("Ensure the session_id is a valid 36-character UUID (e.g., '60c598e2-d602-4e07-a328-c458006d48c7')".to_string());
        out.push("Create a new session using the 'session' tool with action='create' to get a valid UUID".to_string());
        if let Some(Value::String(s)) = received
            && s.len() != 36
        {
            out.push(format!("Your session_id '{}' has {} characters, but UUIDs require exactly 36 characters with hyphens.", s, s.len()));
        }
    }
    if field == "interaction_type" {
        out.push(format!(
            "Valid interaction_type values are: {}",
            VALID_INTERACTION_TYPES.join(", ")
        ));
        out.push("Use lowercase with underscores, not CamelCase or spaces".to_string());
        out.push("Examples: ✅ 'decision_made' ❌ 'DecisionMade' ❌ 'made decision'".to_string());
    }
    if field == "content" && problem == Problem::Missing {
        out.push(
            "For single update mode, provide both 'interaction_type' and 'content' parameters"
                .to_string(),
        );
        out.push("For bulk updates, use 'updates' array instead".to_string());
        out.push("Content must be an object with key-value pairs".to_string());
    }
    if problem == Problem::WrongType {
        out.push(format!("Parameter '{field}' has an incorrect type"));
        match received {
            Some(Value::Number(n)) => out.push(format!("Convert the number {n} to a string")),
            Some(Value::Bool(b)) => {
                out.push(format!("Convert the boolean {b} to a string ('{b}')"));
            }
            _ => {}
        }
    }
    if problem == Problem::Missing {
        out.push("Check that all required parameters are included in your request".to_string());
        out.push(
            "Review the tool schema to see which parameters are required vs optional".to_string(),
        );
    }
    if problem == Problem::NotFound && field == "session_id" {
        out.push("Create a new session using the 'session' tool with action='create'".to_string());
        out.push("Or use semantic_search to find existing sessions".to_string());
    }
    if problem == Problem::WrongType && field.starts_with("updates") {
        out.push("When using bulk mode, 'updates' must be an array of update objects".to_string());
        out.push(
            "Each update in the array must have 'interaction_type' and 'content' fields"
                .to_string(),
        );
    }
    if out.is_empty() {
        out.push("Review the error message and check your parameter types and values".to_string());
        out.push("Use dry_run=true to validate your request without making changes".to_string());
        out.push("Check the tool documentation for the correct parameter format".to_string());
    }
    out
}

#[cfg(test)]
mod tests;
