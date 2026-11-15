// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! Tool arguments into their typed request.
//!
//! Deserialization tries the arguments as given first, then once more
//! after narrow coercions that help agents passing slightly wrong types:
//!
//! - `content` maps (top-level and inside each `updates[]` item) and the
//!   query tool's `parameters` map are `HashMap<String, String>`, so every
//!   non-string value in them becomes a string: numbers and booleans by
//!   `to_string`, objects and arrays as compact JSON;
//! - top-level `*_id` fields are strings, so a number or boolean there
//!   becomes its string form.
//!
//! Typed fields (`limit`, `dry_run`, `updates`, `select_map`, ...) are never
//! rewritten. A failure names the argument at fault from the typed error
//! (the path serde reports, or the required field the schema lists and the
//! arguments lack), never from the error text.

use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::tools::Failure;
use crate::tools::check::Problem;

/// What to tell a caller about one argument of a tool: what it must be and
/// how to fix it.
pub struct FieldHint {
    /// The argument.
    pub field: &'static str,
    /// What it must be.
    pub expected: &'static str,
    /// How to fix it.
    pub hint: &'static str,
}

/// `value` as `T`, coerced where it helps; a failure names the argument at
/// fault and carries its hint from `hints`.
pub fn arguments<T: DeserializeOwned + JsonSchema>(
    value: Value,
    hints: &[FieldHint],
) -> Result<T, Failure> {
    let failure = match coerce_and_validate::<T>(value.clone()) {
        Ok(request) => return Ok(request),
        Err(failure) => failure,
    };
    let (field, problem) = match failure.details.parameter.clone() {
        Some(path) => (path, Problem::WrongType),
        None => match missing_required::<T>(&value) {
            Some(field) => (field, Problem::Missing),
            None => return Err(failure),
        },
    };
    let received = (problem == Problem::WrongType)
        .then(|| value_at(&value, &field).cloned())
        .flatten();
    let mut failure = failure.parameter(field.clone());
    if let Some(h) = hints.iter().find(|h| h.field == field) {
        failure = failure.expected(h.expected).hint(h.hint);
    }
    if let Some(received) = received.clone() {
        failure = failure.received(received);
    }
    Err(failure.suggestions(crate::tools::check::suggestions(
        &field,
        problem,
        received.as_ref(),
    )))
}

/// `value` as `T`, with the coercions above; a failure carries the path of
/// the argument serde stopped at, when it is not the root.
///
/// # Errors
/// The value does not deserialize as `T`, even coerced.
pub fn coerce_and_validate<T: DeserializeOwned>(value: Value) -> Result<T, Failure> {
    // Fast path: deserialize straight from the borrowed value.
    if let Ok(result) = T::deserialize(&value) {
        return Ok(result);
    }
    let coerced = apply_coercions(value)?;
    serde_path_to_error::deserialize::<_, T>(coerced).map_err(|e| {
        let path = e.path().to_string();
        let failure = Failure::invalid(format!(
            "Failed to deserialize parameter(s): {}",
            e.into_inner()
        ));
        if path == "." || path.is_empty() {
            failure
        } else {
            failure.parameter(path)
        }
    })
}

/// The first field the schema of `T` requires that `value` lacks.
fn missing_required<T: JsonSchema>(value: &Value) -> Option<String> {
    let schema = schemars::schema_for!(T);
    let required = schema.get("required")?.as_array()?;
    required
        .iter()
        .filter_map(Value::as_str)
        .find(|field| value.get(field).is_none_or(Value::is_null))
        .map(str::to_string)
}

/// The value at a serde path (`updates[0].content`), if present.
fn value_at<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').try_fold(value, |v, segment| {
        let (name, index) = match segment.split_once('[') {
            Some((name, rest)) => (name, rest.trim_end_matches(']').parse::<usize>().ok()),
            None => (segment, None),
        };
        let v = if name.is_empty() { v } else { v.get(name)? };
        match index {
            Some(i) => v.get(i),
            None => Some(v),
        }
    })
}

/// Apply the coercion rules (module docs) in place.
fn apply_coercions(mut value: Value) -> Result<Value, Failure> {
    let Some(obj) = value.as_object_mut() else {
        return Ok(value);
    };
    for (key, val) in obj.iter_mut() {
        match key.as_str() {
            // Both are `HashMap<String, String>` in the request types.
            "content" | "parameters" => coerce_string_map(val)?,
            "updates" => {
                if let Value::Array(items) = val {
                    for item in items {
                        if let Some(content) = item.get_mut("content") {
                            coerce_string_map(content)?;
                        }
                    }
                }
            }
            k if k.ends_with("_id") => coerce_scalar_to_string(val),
            _ => {}
        }
    }
    Ok(value)
}

/// Stringify every non-string value of a `HashMap<String, String>`-shaped object.
fn coerce_string_map(val: &mut Value) -> Result<(), Failure> {
    if let Value::Object(map) = val {
        for v in map.values_mut() {
            coerce_to_string(v)?;
        }
    }
    Ok(())
}

/// Number/Boolean → String; everything else is left untouched.
fn coerce_scalar_to_string(val: &mut Value) {
    match val {
        Value::Number(n) => *val = Value::String(n.to_string()),
        Value::Bool(b) => *val = Value::String(b.to_string()),
        _ => {}
    }
}

/// Number/Boolean → String, Object/Array → compact JSON string. Strings and
/// nulls pass through unchanged.
fn coerce_to_string(val: &mut Value) -> Result<(), Failure> {
    match val {
        Value::Number(_) | Value::Bool(_) => coerce_scalar_to_string(val),
        Value::Object(_) | Value::Array(_) => {
            let json = serde_json::to_string(val).map_err(|e| {
                Failure::invalid(format!(
                    "Failed to serialize nested value to JSON string: {e}"
                ))
                .received(val.clone())
            })?;
            *val = Value::String(json);
        }
        Value::String(_) | Value::Null => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests;
