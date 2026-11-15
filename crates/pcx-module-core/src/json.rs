// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! JSON values to and from `google.protobuf.Value`, by the proto3 JSON
//! mapping.

use pbjson_types::value::Kind;
use pbjson_types::{ListValue, Struct};
use serde_json::{Map, Number, Value};

/// The largest integer a double holds exactly (2^53).
const MAX_EXACT: f64 = 9_007_199_254_740_992.0;

/// `value` as a `google.protobuf.Value`: numbers become doubles, a number
/// no double holds its decimal string (as proto3 JSON writes int64).
pub fn to_wire(value: Value) -> pbjson_types::Value {
    let kind = match value {
        Value::Null => Kind::NullValue(0),
        Value::Bool(b) => Kind::BoolValue(b),
        Value::Number(n) => match n.as_f64() {
            Some(n) => Kind::NumberValue(n),
            None => Kind::StringValue(n.to_string()),
        },
        Value::String(s) => Kind::StringValue(s),
        Value::Array(items) => Kind::ListValue(ListValue {
            values: items.into_iter().map(to_wire).collect(),
        }),
        Value::Object(fields) => Kind::StructValue(object_to_wire(fields)),
    };
    pbjson_types::Value { kind: Some(kind) }
}

/// A `google.protobuf.Value` as JSON. An unset kind is null; a whole number
/// a double holds exactly is an integer (as proto3 JSON writes it); a number
/// JSON cannot hold (NaN, infinity) is its proto3 JSON string.
pub fn from_wire(value: pbjson_types::Value) -> Value {
    match value.kind {
        None | Some(Kind::NullValue(_)) => Value::Null,
        Some(Kind::BoolValue(b)) => Value::Bool(b),
        Some(Kind::NumberValue(n)) if n.fract() == 0.0 && n.abs() <= MAX_EXACT => {
            // Exact: a whole double within 2^53 is that integer.
            Value::Number(Number::from(n as i64))
        }
        Some(Kind::NumberValue(n)) => match Number::from_f64(n) {
            Some(n) => Value::Number(n),
            None if n.is_nan() => Value::String("NaN".to_string()),
            None if n > 0.0 => Value::String("Infinity".to_string()),
            None => Value::String("-Infinity".to_string()),
        },
        Some(Kind::StringValue(s)) => Value::String(s),
        Some(Kind::ListValue(list)) => {
            Value::Array(list.values.into_iter().map(from_wire).collect())
        }
        Some(Kind::StructValue(fields)) => Value::Object(object_from_wire(fields)),
    }
}

/// A `google.protobuf.Struct` as a JSON object.
pub fn object_from_wire(fields: Struct) -> Map<String, Value> {
    fields
        .fields
        .into_iter()
        .map(|(k, v)| (k, from_wire(v)))
        .collect()
}

/// A JSON object as a `google.protobuf.Struct`.
pub fn object_to_wire(fields: Map<String, Value>) -> Struct {
    Struct {
        fields: fields.into_iter().map(|(k, v)| (k, to_wire(v))).collect(),
    }
}

/// `value` without the members that are null.
fn without_nulls(value: Value) -> Value {
    match value {
        Value::Object(members) => Value::Object(
            members
                .into_iter()
                .filter(|(_, member)| !member.is_null())
                .map(|(name, member)| (name, without_nulls(member)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.into_iter().map(without_nulls).collect()),
        other => other,
    }
}

/// An outside API's answer `body` as `T`, a mirror message read by the
/// proto3 JSON mapping. Such an API (Nomad, Discord) writes an empty list,
/// map, time or text as null, which in that mapping is the field left out:
/// its null members are dropped first.
///
/// # Errors
/// `body` is no JSON, or not what `T` reads.
pub fn decode<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, serde_json::Error> {
    decode_value(serde_json::from_slice(body)?)
}

/// As [`decode`], of JSON already read (a part of a frame).
///
/// # Errors
/// `value` is not what `T` reads.
pub fn decode_value<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, serde_json::Error> {
    T::deserialize(without_nulls(value))
}

/// A tool's input as `T`: an unknown field, a missing one or one of
/// another type is `InvalidArgument`, naming it.
///
/// # Errors
/// `InvalidArgument` when `input` does not fit `T`.
pub fn input<T: serde::de::DeserializeOwned>(input: Value) -> Result<T, crate::ModuleError> {
    serde_json::from_value(input).map_err(|e| {
        crate::ModuleError::new(
            crate::ModuleErrorKind::InvalidArgument,
            format!("the input: {e}"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::decode;

    #[derive(serde::Deserialize, Debug, PartialEq)]
    struct Task {
        name: String,
        #[serde(default)]
        events: Vec<String>,
    }

    #[test]
    fn an_outside_apis_nulls_are_fields_left_out() {
        let tasks: Vec<Task> =
            decode(br#"[{"name":"cloud","events":null},{"name":"modules","events":["Started"]}]"#)
                .unwrap();
        assert_eq!(tasks[0].events, Vec::<String>::new());
        assert_eq!(tasks[1].events, ["Started"]);
        let none: Option<Task> = decode(b"null").unwrap();
        assert_eq!(none, None);
        assert!(decode::<Task>(b"not json").is_err());
    }
}
