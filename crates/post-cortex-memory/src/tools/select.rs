// Copyright (c) 2026 Julius ML
// MIT License

//! Minimal JSONPath subset parser and executor for the `select` parameter
//! on `AssembleContext`.
//!
//! **Allowed syntax:**
//! - `$` (root)
//! - `.field` (child access)
//! - `[n]` / `[-n]` (index, including negative from end)
//! - `[a:b]` (slice, no step)
//! - `*` or `[*]` (wildcard)
//! - `[0,3,7]` (union of indices)
//!
//! **Forbidden (parse-time error with line+col):**
//! - `..` recursive descent
//! - `[?(...)]` filter expressions
//! - functions: `length()`, `match()`, `search()`, `count()`
//! - `[a:b:c]` step in slice
//!
//! The one `select` of every transport: it runs over the serde form of an
//! operation's response (see [`crate::tools::responses`]).

use std::collections::HashMap;

use serde::Serialize;
use serde_json::Value;

use crate::tools::Failure;

mod parse;

pub use parse::ParseError;
use parse::{JsonPath, PathSegment, parse};

// ---------------------------------------------------------------------------
// Executor
// ---------------------------------------------------------------------------

/// Execute a parsed [`JsonPath`] against a JSON [`Value`].
///
/// Returns the matched value. If nothing matches, returns `Value::Null`.
fn execute(path: &JsonPath, root: &Value) -> Value {
    // Walk by reference; only the final matches are cloned.
    let mut current: Vec<&Value> = vec![root];

    for segment in &path.segments {
        let mut next = Vec::new();
        for val in current {
            match segment {
                PathSegment::Field(name) => {
                    if let Some(child) = val.get(name) {
                        next.push(child);
                    }
                }
                PathSegment::Index(idx) => {
                    if let Some(v) = index_value(val, *idx) {
                        next.push(v);
                    }
                }
                PathSegment::Slice { start, end } => {
                    next.extend(slice_value(val, *start, *end));
                }
                PathSegment::Wildcard => match val {
                    Value::Array(arr) => next.extend(arr),
                    Value::Object(map) => next.extend(map.values()),
                    _ => {}
                },
                PathSegment::Union(indices) => {
                    for &idx in indices {
                        if let Some(v) = index_value(val, idx) {
                            next.push(v);
                        }
                    }
                }
            }
        }
        current = next;
    }

    match current.as_slice() {
        [] => Value::Null,
        [single] => (*single).clone(),
        many => Value::Array(many.iter().map(|v| (*v).clone()).collect()),
    }
}

/// Apply a `select` specification against a JSON response (see
/// [`Select::apply`] for a typed response).
///
/// **Form A** (`select_string`): single JSONPath → returns the matched value
/// directly (or `Null` if nothing matched).
///
/// **Form B** (`select_map`): map of output-key → JSONPath → returns an
/// object with the same keys, each containing the match result.
pub fn apply_select(
    response_json: &Value,
    select_string: Option<&str>,
    select_map: Option<&std::collections::HashMap<String, String>>,
) -> Result<Value, ParseError> {
    if let Some(expr) = select_string {
        let path = parse(expr)?;
        Ok(execute(&path, response_json))
    } else if let Some(map) = select_map {
        let mut result = serde_json::Map::with_capacity(map.len());
        for (key, expr) in map {
            let path = parse(expr)?;
            let val = execute(&path, response_json);
            result.insert(key.clone(), val);
        }
        Ok(Value::Object(result))
    } else {
        Ok(response_json.clone())
    }
}

// ---------------------------------------------------------------------------
// Executor helpers
// ---------------------------------------------------------------------------

fn index_value(val: &Value, idx: isize) -> Option<&Value> {
    match val {
        Value::Array(arr) => {
            if idx >= 0 {
                arr.get(idx as usize)
            } else {
                let from_end = idx.unsigned_abs();
                if from_end <= arr.len() {
                    arr.get(arr.len() - from_end)
                } else {
                    None
                }
            }
        }
        _ => None,
    }
}

fn slice_value(val: &Value, start: Option<isize>, end: Option<isize>) -> &[Value] {
    match val {
        Value::Array(arr) => {
            let len = arr.len() as isize;
            let s = start.unwrap_or(0);
            let s = if s < 0 { (len + s).max(0) } else { s } as usize;
            let e = end.unwrap_or(len);
            let e = if e < 0 { (len + e).max(0) } else { e.min(len) } as usize;
            if s >= e { &[] } else { &arr[s..e] }
        }
        _ => &[],
    }
}

// ---------------------------------------------------------------------------

/// What JSONPath syntax `select` allows, for error hints.
pub const SELECT_SYNTAX: &str = "Allowed: $, .field, [n], [-n], [a:b], [*], [0,3,7]. \
                                 Forbidden: .., [?(...)], functions, [a:b:c]";

/// A request's `select`: one JSONPath (form A) or a map of output keys to
/// JSONPaths (form B); neither selects the whole response.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Select {
    /// Form A.
    pub path: Option<String>,
    /// Form B.
    pub map: Option<HashMap<String, String>>,
}

impl Select {
    /// A select of form A and/or B (A wins when both are given).
    #[must_use]
    pub fn new(path: Option<String>, map: Option<HashMap<String, String>>) -> Self {
        Self {
            path,
            map: map.filter(|m| !m.is_empty()),
        }
    }

    /// Selects the whole response.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.path.is_none() && self.map.is_none()
    }

    /// The part of `response` (in its serde form) this select picks.
    ///
    /// # Errors
    /// An expression is invalid (`select` is the argument at fault), or the
    /// response is not serialisable.
    pub fn apply<T: Serialize>(&self, response: &T) -> Result<Value, Failure> {
        let value = serde_json::to_value(response)
            .map_err(|e| Failure::internal(format!("response not serialisable: {e}")))?;
        if self.is_empty() {
            return Ok(value);
        }
        apply_select(&value, self.path.as_deref(), self.map.as_ref()).map_err(|e| {
            let mut failure =
                Failure::invalid(format!("Invalid JSONPath expression: {}", e.message))
                    .parameter(if self.path.is_some() {
                        "select"
                    } else {
                        "select_map"
                    })
                    .hint(SELECT_SYNTAX);
            failure.details.offset = Some(e.byte_offset);
            failure
        })
    }
}

#[cfg(test)]
mod tests;
