// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The values expressions take and give: read from JSON by their type, and
//! converted to and from CEL in this one place. Text, bytes, lists and maps
//! are shared as CEL holds them (`shared`), so a clone and a value handed
//! to CEL copy a pointer, not the items (docs/modules/workflow.md, data
//! blocks, speed).

use std::collections::BTreeMap;
use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use cel_core_proto::ProstMessage;
use chrono::{DateTime, SecondsFormat, TimeDelta, Utc};
use prost_reflect::{DynamicMessage, ReflectMessage, SerializeOptions};
use serde_json::Value as Json;

use crate::types::Type;

pub use shared::{Entries, List};

/// A value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// The null value.
    Null,
    /// Text.
    String(Arc<str>),
    /// A signed whole number.
    Int(i64),
    /// An unsigned whole number.
    Uint(u64),
    /// A number.
    Double(f64),
    /// True or false.
    Bool(bool),
    /// Bytes.
    Bytes(Arc<[u8]>),
    /// A time.
    Timestamp(DateTime<Utc>),
    /// A duration.
    Duration(TimeDelta),
    /// A list.
    List(List),
    /// A map from text.
    Map(Entries),
    /// A proto message.
    Message(DynamicMessage),
}

/// Values by name: a template's inputs, or the variables of an
/// expression (`input.name`, `steps.fetch`).
pub type Values = BTreeMap<String, Value>;

impl Value {
    /// `json` read as a value of `ty`: a string, a whole number (`int`,
    /// `uint`), a number, a bool, base64 (`bytes`), an RFC 3339 time
    /// (`timestamp`), seconds with an `s` (`duration`, `"1.5s"`), an array
    /// or an object of them; as `dyn`, any JSON as it reads (null, a bool,
    /// a whole number as `int`, else a `double`, a string, a list or a map
    /// of `dyn`).
    ///
    /// # Errors
    /// It is not one, or `ty` is one JSON does not carry here (a message,
    /// `null`); the reason.
    pub fn read(ty: &Type, json: &Json) -> Result<Self, String> {
        let refused = || format!("{json} is not a {ty}");
        Ok(match (ty, json) {
            (Type::String, Json::String(s)) => Self::String(Arc::from(s.as_str())),
            (Type::Int, Json::Number(n)) => Self::Int(whole(n).ok_or_else(refused)?),
            (Type::Uint, Json::Number(n)) => Self::Uint(
                whole(n)
                    .and_then(|i| u64::try_from(i).ok())
                    .or_else(|| n.as_u64())
                    .ok_or_else(refused)?,
            ),
            (Type::Double, Json::Number(n)) => Self::Double(n.as_f64().ok_or_else(refused)?),
            (Type::Bool, Json::Bool(b)) => Self::Bool(*b),
            (Type::Bytes, Json::String(s)) => Self::Bytes(
                BASE64
                    .decode(s)
                    .map_err(|_| format!("\"{s}\" is not base64"))?
                    .into(),
            ),
            (Type::Timestamp, Json::String(s)) => Self::Timestamp(
                DateTime::parse_from_rfc3339(s)
                    .map_err(|_| format!("\"{s}\" is not an RFC 3339 time"))?
                    .with_timezone(&Utc),
            ),
            (Type::Duration, Json::String(s)) => Self::Duration(
                duration(s)
                    .ok_or_else(|| format!("\"{s}\" is not a duration (seconds, then s)"))?,
            ),
            (Type::List(item), Json::Array(items)) => Self::List(List::new(
                items
                    .iter()
                    .map(|i| Self::read(item, i))
                    .collect::<Result<Vec<_>, _>>()?,
            )),
            (Type::Map(value), Json::Object(entries)) => Self::Map(Entries::new(
                entries
                    .iter()
                    .map(|(k, v)| Ok((k.clone(), Self::read(value, v)?)))
                    .collect::<Result<Vec<_>, String>>()?,
            )),
            (Type::Dyn, Json::Null) => Self::Null,
            (Type::Dyn, Json::Bool(b)) => Self::Bool(*b),
            (Type::Dyn, Json::Number(n)) => match whole(n) {
                Some(i) => Self::Int(i),
                None => Self::Double(n.as_f64().ok_or_else(refused)?),
            },
            (Type::Dyn, Json::String(s)) => Self::String(Arc::from(s.as_str())),
            (Type::Dyn, Json::Array(_)) => Self::read(&Type::List(Box::new(Type::Dyn)), json)?,
            (Type::Dyn, Json::Object(_)) => Self::read(&Type::Map(Box::new(Type::Dyn)), json)?,
            _ => return Err(refused()),
        })
    }

    /// Its type: a list's or a map's of its first item (`dyn` with none).
    #[must_use]
    pub fn ty(&self) -> Type {
        match self {
            Self::Null => Type::Null,
            Self::String(_) => Type::String,
            Self::Int(_) => Type::Int,
            Self::Uint(_) => Type::Uint,
            Self::Double(_) => Type::Double,
            Self::Bool(_) => Type::Bool,
            Self::Bytes(_) => Type::Bytes,
            Self::Timestamp(_) => Type::Timestamp,
            Self::Duration(_) => Type::Duration,
            Self::List(items) => Type::List(Box::new(match items.get(0) {
                Ok(first) => first.ty(),
                Err(_) => Type::Dyn,
            })),
            Self::Map(entries) => Type::Map(Box::new(match entries.iter().next() {
                Some(Ok((_, first))) => first.ty(),
                _ => Type::Dyn,
            })),
            Self::Message(m) => Type::of_message(m.descriptor().full_name()),
        }
    }

    /// The value as JSON, as [`Value::read`] reads it back: bytes as
    /// base64, a time as RFC 3339, a duration as seconds with an `s`, a
    /// message by the canonical proto JSON mapping with proto field names.
    ///
    /// # Errors
    /// A double JSON cannot carry (NaN, infinite), a message that does not
    /// map; the reason.
    pub fn json(&self) -> Result<Json, String> {
        Ok(match self {
            Self::Null => Json::Null,
            Self::Bool(b) => Json::Bool(*b),
            Self::Int(i) => Json::from(*i),
            Self::Uint(u) => Json::from(*u),
            Self::Double(d) => serde_json::Number::from_f64(*d)
                .map(Json::Number)
                .ok_or_else(|| format!("{d} has no JSON"))?,
            Self::String(s) => Json::String(s.to_string()),
            Self::Bytes(b) => Json::String(BASE64.encode(b)),
            Self::Timestamp(t) => Json::String(t.to_rfc3339_opts(SecondsFormat::AutoSi, true)),
            Self::Duration(_) => {
                Json::String(text(&self.cel()).ok_or_else(|| "a duration has no text".to_string())?)
            }
            Self::List(items) => {
                Json::Array(items.iter().map(|i| i?.json()).collect::<Result<_, _>>()?)
            }
            Self::Map(entries) => Json::Object(
                entries
                    .iter()
                    .map(|e| {
                        let (k, v) = e?;
                        Ok((k.to_string(), v.json()?))
                    })
                    .collect::<Result<_, String>>()?,
            ),
            Self::Message(m) => {
                let options = SerializeOptions::new().use_proto_field_name(true);
                m.serialize_with_options(serde_json::value::Serializer, &options)
                    .map_err(|e| {
                        format!("a {} does not map to JSON: {e}", m.descriptor().full_name())
                    })?
            }
        })
    }

    /// The value as CEL evaluates it.
    pub fn cel(&self) -> cel_core::Value {
        match self {
            Self::Null => cel_core::Value::Null,
            Self::String(s) => cel_core::Value::String(Arc::clone(s)),
            Self::Int(i) => cel_core::Value::Int(*i),
            Self::Uint(u) => cel_core::Value::UInt(*u),
            Self::Double(d) => cel_core::Value::Double(*d),
            Self::Bool(b) => cel_core::Value::Bool(*b),
            Self::Bytes(b) => cel_core::Value::Bytes(Arc::clone(b)),
            Self::Timestamp(t) => cel_core::Value::Timestamp(cel_core::Timestamp::from_datetime(t)),
            Self::Duration(d) => cel_core::Value::Duration(cel_core::Duration::new(
                d.num_seconds(),
                d.subsec_nanos(),
            )),
            Self::List(items) => items.cel(),
            Self::Map(entries) => entries.cel(),
            Self::Message(m) => cel_core::Value::Message(Box::new(ProstMessage::new(m.clone()))),
        }
    }

    /// `value`, an expression's result, as a value of this crate: a list
    /// or a map shared, not copied.
    ///
    /// # Errors
    /// It is one no value here is (an optional, a type, a map not from
    /// text, a message not read by the descriptors, an error); the reason.
    pub fn of_cel(value: &cel_core::Value) -> Result<Self, String> {
        Ok(match value {
            cel_core::Value::Null => Self::Null,
            cel_core::Value::Bool(b) => Self::Bool(*b),
            cel_core::Value::Int(i) => Self::Int(*i),
            cel_core::Value::UInt(u) => Self::Uint(*u),
            cel_core::Value::Double(d) => Self::Double(*d),
            cel_core::Value::String(s) => Self::String(Arc::clone(s)),
            cel_core::Value::Bytes(b) => Self::Bytes(Arc::clone(b)),
            cel_core::Value::Timestamp(t) => Self::Timestamp(
                t.to_datetime_utc()
                    .ok_or_else(|| "a time out of range".to_string())?,
            ),
            cel_core::Value::Duration(d) => Self::Duration(
                TimeDelta::try_seconds(d.seconds)
                    .and_then(|s| s.checked_add(&TimeDelta::nanoseconds(d.nanos.into())))
                    .ok_or_else(|| "a duration out of range".to_string())?,
            ),
            cel_core::Value::List(items) => Self::List(List::of_cel(items)?),
            cel_core::Value::Map(map) => Self::Map(Entries::of_cel(map)?),
            cel_core::Value::Enum(e) => Self::Int(e.value.into()),
            cel_core::Value::Message(m) => Self::Message(
                m.as_any()
                    .downcast_ref::<ProstMessage>()
                    .ok_or_else(|| format!("a {} not read by the descriptors", m.type_name()))?
                    .message()
                    .clone(),
            ),
            cel_core::Value::Error(e) => return Err(e.message.clone()),
            other => {
                return Err(format!(
                    "a {} is no value here",
                    other.cel_type().display_name()
                ));
            }
        })
    }

    /// Whether `value` is held here as CEL holds it (true), or holds an
    /// enum, which is a number here, so a list or a map of it is made again
    /// (false).
    ///
    /// # Errors
    /// It is none here; why.
    pub fn plain(value: &cel_core::Value) -> Result<bool, String> {
        Ok(match value {
            cel_core::Value::Null
            | cel_core::Value::Bool(_)
            | cel_core::Value::Int(_)
            | cel_core::Value::UInt(_)
            | cel_core::Value::Double(_)
            | cel_core::Value::String(_)
            | cel_core::Value::Bytes(_)
            | cel_core::Value::Duration(_) => true,
            cel_core::Value::Timestamp(t) => {
                t.to_datetime_utc()
                    .ok_or_else(|| "a time out of range".to_string())?;
                true
            }
            cel_core::Value::Enum(_) => false,
            cel_core::Value::List(items) => {
                let mut plain = true;
                for item in items.iter() {
                    plain &= Self::plain(item)?;
                }
                plain
            }
            cel_core::Value::Map(map) => {
                let mut plain = true;
                for (k, v) in map.iter() {
                    if !matches!(k, cel_core::MapKey::String(_)) {
                        return Err("a map whose keys are not text".to_string());
                    }
                    plain &= Self::plain(v)?;
                }
                plain
            }
            cel_core::Value::Message(m) if m.as_any().is::<ProstMessage>() => true,
            other => {
                return Err(format!(
                    "a {} is no value here",
                    other.cel_type().display_name()
                ));
            }
        })
    }

    /// Text `text`.
    pub fn string(text: impl Into<Arc<str>>) -> Self {
        Self::String(text.into())
    }

    /// The list of `items`.
    pub fn list(items: impl IntoIterator<Item = Value>) -> Self {
        Self::List(List::new(items))
    }

    /// The map of `entries`.
    pub fn map(entries: impl IntoIterator<Item = (String, Value)>) -> Self {
        Self::Map(Entries::new(entries))
    }
}

/// `n` as a whole number: an integer, or a number with no fraction in
/// range (`google.protobuf.Value` carries every number as a double, so a
/// whole number sent through it reads as `3.0`).
fn whole(n: &serde_json::Number) -> Option<i64> {
    if let Some(i) = n.as_i64() {
        return Some(i);
    }
    let d = n.as_f64()?;
    // i64's range as doubles: [-2^63, 2^63).
    let fits = d.fract() == 0.0
        && (-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0).contains(&d);
    // A whole double in i64's range converts exactly.
    fits.then_some(d as i64)
}

/// `text`, proto JSON's duration: seconds with up to 9 decimals, then `s`.
fn duration(text: &str) -> Option<TimeDelta> {
    let number = text.strip_suffix('s')?;
    let (negative, number) = match number.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, number),
    };
    let (seconds, fraction) = number.split_once('.').unwrap_or((number, ""));
    if seconds.is_empty() || fraction.len() > 9 || !fraction.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let seconds: i64 = seconds.parse().ok()?;
    let nanos: u32 = if fraction.is_empty() {
        0
    } else {
        format!("{fraction:0<9}").parse().ok()?
    };
    let d = TimeDelta::new(seconds, nanos)?;
    Some(if negative { -d } else { d })
}

/// `value`, an expression's result, as text shows it; none when text
/// cannot show it (a list, a map, bytes, null).
pub fn text(value: &cel_core::Value) -> Option<String> {
    Some(match value {
        cel_core::Value::String(s) => s.to_string(),
        cel_core::Value::Int(i) => i.to_string(),
        cel_core::Value::UInt(u) => u.to_string(),
        cel_core::Value::Double(d) => d.to_string(),
        cel_core::Value::Bool(b) => b.to_string(),
        cel_core::Value::Timestamp(t) => t
            .to_datetime_utc()?
            .to_rfc3339_opts(SecondsFormat::AutoSi, true),
        cel_core::Value::Duration(d) => {
            let nanos = d.to_nanos();
            let sign = if nanos < 0 { "-" } else { "" };
            let nanos = nanos.unsigned_abs();
            let (seconds, fraction) = (nanos / 1_000_000_000, nanos % 1_000_000_000);
            if fraction == 0 {
                format!("{sign}{seconds}s")
            } else {
                let fraction = format!("{fraction:09}");
                format!("{sign}{seconds}.{}s", fraction.trim_end_matches('0'))
            }
        }
        _ => return None,
    })
}

mod proto;

mod shared;

#[cfg(test)]
mod tests;
