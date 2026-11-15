// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A proto field's value as a value here, as CEL reads it: numbers by
//! their kind, an enum as its number, a time and a duration as theirs,
//! another message as itself.

use chrono::{DateTime, TimeDelta};
use prost_reflect::{
    DynamicMessage, Kind, MapKey, ReflectMessage, SerializeOptions, Value as Proto,
};

use crate::types::Type;
use crate::value::{Entries, Value};

/// The well-known time.
const TIMESTAMP: &str = "google.protobuf.Timestamp";

/// The well-known duration.
const DURATION: &str = "google.protobuf.Duration";

/// The well-known JSON messages: a value, a struct, a list.
const JSON: [&str; 3] = [
    "google.protobuf.Value",
    "google.protobuf.Struct",
    "google.protobuf.ListValue",
];

/// The well-known wrappers, each of one field `value`.
const WRAPPERS: [&str; 9] = [
    "google.protobuf.StringValue",
    "google.protobuf.BoolValue",
    "google.protobuf.Int32Value",
    "google.protobuf.Int64Value",
    "google.protobuf.UInt32Value",
    "google.protobuf.UInt64Value",
    "google.protobuf.FloatValue",
    "google.protobuf.DoubleValue",
    "google.protobuf.BytesValue",
];

impl Value {
    /// The fields of `m` as a map, by their names: what a reader without its
    /// descriptors can read.
    ///
    /// # Errors
    /// As [`Value::of_proto`].
    pub fn fields_of(m: &DynamicMessage) -> Result<Self, String> {
        Ok(Self::Map(
            m.descriptor()
                .fields()
                .map(|field| {
                    let value = m.get_field(&field);
                    Ok((
                        field.name().to_string(),
                        Self::of_proto(&field.kind(), &value)?,
                    ))
                })
                .collect::<Result<Entries, String>>()?,
        ))
    }

    /// `value`, a proto field's of `kind` (a repeated field's list, a map
    /// field's map), as a value here.
    ///
    /// # Errors
    /// A map whose keys are not text, a time or a duration out of range;
    /// why.
    pub fn of_proto(kind: &Kind, value: &Proto) -> Result<Self, String> {
        Ok(match value {
            Proto::Bool(b) => Self::Bool(*b),
            Proto::I32(i) => Self::Int((*i).into()),
            Proto::I64(i) => Self::Int(*i),
            Proto::U32(u) => Self::Uint((*u).into()),
            Proto::U64(u) => Self::Uint(*u),
            Proto::F32(f) => Self::Double((*f).into()),
            Proto::F64(f) => Self::Double(*f),
            Proto::String(s) => Self::string(s.as_str()),
            Proto::Bytes(b) => Self::Bytes(b.to_vec().into()),
            Proto::EnumNumber(n) => Self::Int((*n).into()),
            Proto::Message(m) => message(m)?,
            Proto::List(items) => Self::List(
                items
                    .iter()
                    .map(|i| Self::of_proto(kind, i))
                    .collect::<Result<_, _>>()?,
            ),
            Proto::Map(entries) => {
                let value_kind = match kind {
                    Kind::Message(entry) if entry.is_map_entry() => {
                        entry.map_entry_value_field().kind()
                    }
                    other => other.clone(),
                };
                Self::Map(
                    entries
                        .iter()
                        .map(|(k, v)| match k {
                            MapKey::String(k) => Ok((k.clone(), Self::of_proto(&value_kind, v)?)),
                            _ => Err("a map whose keys are not text".to_string()),
                        })
                        .collect::<Result<Entries, String>>()?,
                )
            }
        })
    }
}

/// `m` as CEL reads it: a time, a duration, a JSON value, struct or list
/// and a wrapper as the value its type ([`Type::of_message`]) says (a
/// JSON value with no kind is null), else the message.
fn message(m: &DynamicMessage) -> Result<Value, String> {
    let descriptor = m.descriptor();
    let name = descriptor.full_name();
    if JSON.contains(&name) {
        let json = m
            .serialize_with_options(serde_json::value::Serializer, &SerializeOptions::new())
            .map_err(|e| format!("a {name} does not map to JSON: {e}"))?;
        return Value::read(&Type::of_message(name), &json);
    }
    if WRAPPERS.contains(&name) {
        let field = descriptor
            .get_field_by_name("value")
            .ok_or_else(|| format!("a {name} without its value"))?;
        return Value::of_proto(&field.kind(), &m.get_field(&field));
    }
    timed(m)
}

/// `m` as CEL reads it: a time, a duration, else the message.
fn timed(m: &DynamicMessage) -> Result<Value, String> {
    let int = |name: &str| match m.get_field_by_name(name).as_deref() {
        Some(Proto::I64(i)) => Ok(*i),
        Some(Proto::I32(i)) => Ok((*i).into()),
        _ => Err(format!(
            "a {} without its {name}",
            m.descriptor().full_name()
        )),
    };
    Ok(match m.descriptor().full_name() {
        TIMESTAMP => {
            let nanos = u32::try_from(int("nanos")?).map_err(|_| "a time's nanos below 0")?;
            Value::Timestamp(
                DateTime::from_timestamp(int("seconds")?, nanos)
                    .ok_or_else(|| "a time out of range".to_string())?,
            )
        }
        DURATION => {
            let nanos = TimeDelta::nanoseconds(int("nanos")?);
            Value::Duration(
                TimeDelta::try_seconds(int("seconds")?)
                    .and_then(|s| s.checked_add(&nanos))
                    .ok_or_else(|| "a duration out of range".to_string())?,
            )
        }
        _ => Value::Message(m.clone()),
    })
}
