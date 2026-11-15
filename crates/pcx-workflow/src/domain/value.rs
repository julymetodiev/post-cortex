// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A workflow's `Value`s (docs/modules/workflow.md, expressions): typed as
//! `pcx-expr` types them (a constant of a template, an example of a
//! schema's field), and read into and written from `pcx-expr`'s values,
//! which CEL evaluates: the one place they convert.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use chrono::{DateTime, TimeDelta};
use pcx_expr::{Type, Value as Expr};
use post_cortex_proto::module::BusEvent;
use post_cortex_proto::workflow::{List, Map, Null, Value, value::Kind};
use prost::{Message, Name};
use prost_reflect::{DescriptorPool, DynamicMessage, ReflectMessage};

/// The prefix of an `Any`'s type URL before the message's full name.
const TYPE_URL: &str = "type.googleapis.com/";

/// `event` with its payload's type URL as CEL reads it. A type's URL is its
/// message's name after the last `/`, whatever comes before it (the modules
/// write `/<name>`), and CEL unpacks an `Any` only when [`TYPE_URL`] comes
/// before it: with another URL `event.payload.<field>` finds no field.
pub fn as_heard(event: &BusEvent) -> BusEvent {
    let mut event = event.clone();
    if let Some(payload) = event.payload.as_mut() {
        let name = payload.type_url.rsplit('/').next().unwrap_or_default();
        payload.type_url = format!("{TYPE_URL}{name}");
    }
    event
}

/// The messages the workflow's own descriptors hold, which its answers carry
/// as they are; a reader of them has no others.
static OWN: LazyLock<Result<DescriptorPool, String>> = LazyLock::new(|| {
    DescriptorPool::decode(post_cortex_proto::WORKFLOW_DESCRIPTOR_SET).map_err(|e| e.to_string())
});

/// The type of `value`: a list's or map's the one type of its items
/// (`dyn` when they differ or there are none), a message's its type URL's
/// message, which `messages` must hold.
///
/// # Errors
/// A value with no kind, or a message `messages` does not hold; the
/// reason.
pub fn type_of(value: &Value, messages: &DescriptorPool) -> Result<Type, String> {
    let kind = value
        .kind
        .as_ref()
        .ok_or_else(|| "a value has a kind (null, bool, int, ...)".to_string())?;
    Ok(match kind {
        Kind::Null(_) => Type::Null,
        Kind::Bool(_) => Type::Bool,
        Kind::Int(_) => Type::Int,
        Kind::Uint(_) => Type::Uint,
        Kind::Double(_) => Type::Double,
        Kind::String(_) => Type::String,
        Kind::Bytes(_) => Type::Bytes,
        Kind::Timestamp(_) => Type::Timestamp,
        Kind::Duration(_) => Type::Duration,
        Kind::List(list) => Type::List(Box::new(one(list.items.iter(), messages)?)),
        Kind::Map(map) => Type::Map(Box::new(one(map.entries.values(), messages)?)),
        Kind::Message(any) => {
            let name = any.type_url.strip_prefix(TYPE_URL).unwrap_or(&any.type_url);
            if messages.get_message_by_name(name).is_none() {
                return Err(format!("{name} is not a message of the modules reached"));
            }
            Type::of_message(name)
        }
    })
}

/// The one type of `values`; `dyn` when they differ or there are none.
fn one<'a>(
    values: impl Iterator<Item = &'a Value>,
    messages: &DescriptorPool,
) -> Result<Type, String> {
    let mut found: Option<Type> = None;
    for value in values {
        let ty = type_of(value, messages)?;
        match &found {
            None => found = Some(ty),
            Some(seen) if *seen == ty => {}
            Some(_) => return Ok(Type::Dyn),
        }
    }
    Ok(found.unwrap_or(Type::Dyn))
}

/// `t` as chrono's time; none out of range (or with negative nanos).
pub fn time(t: &prost_types::Timestamp) -> Option<DateTime<chrono::Utc>> {
    DateTime::from_timestamp(t.seconds, u32::try_from(t.nanos).ok()?)
}

/// `t` as a proto time.
pub fn timestamp(t: DateTime<chrono::Utc>) -> prost_types::Timestamp {
    prost_types::Timestamp {
        seconds: t.timestamp(),
        // Below 2 * 10^9 (a leap second): an i32 holds it.
        nanos: t.timestamp_subsec_nanos() as i32,
    }
}

/// `value` as `pcx-expr` evaluates it, a message read by `messages`.
///
/// # Errors
/// A value with no kind, a time out of range, a message `messages` does
/// not hold or whose bytes do not read; the reason.
pub fn read(value: &Value, messages: &DescriptorPool) -> Result<Expr, String> {
    let kind = value
        .kind
        .as_ref()
        .ok_or_else(|| "a value has a kind (null, bool, int, ...)".to_string())?;
    Ok(match kind {
        Kind::Null(_) => Expr::Null,
        Kind::Bool(b) => Expr::Bool(*b),
        Kind::Int(i) => Expr::Int(*i),
        Kind::Uint(u) => Expr::Uint(*u),
        Kind::Double(d) => Expr::Double(*d),
        Kind::String(s) => Expr::string(s.as_str()),
        Kind::Bytes(b) => Expr::Bytes(b.as_slice().into()),
        Kind::Timestamp(t) => {
            Expr::Timestamp(time(t).ok_or_else(|| "a time out of range".to_string())?)
        }
        Kind::Duration(d) => Expr::Duration(
            TimeDelta::try_seconds(d.seconds)
                .and_then(|s| s.checked_add(&TimeDelta::nanoseconds(d.nanos.into())))
                .ok_or_else(|| "a duration out of range".to_string())?,
        ),
        Kind::List(list) => Expr::List(
            list.items
                .iter()
                .map(|v| read(v, messages))
                .collect::<Result<pcx_expr::List, _>>()?,
        ),
        Kind::Map(map) => Expr::Map(
            map.entries
                .iter()
                .map(|(k, v)| Ok((k.clone(), read(v, messages)?)))
                .collect::<Result<pcx_expr::Entries, String>>()?,
        ),
        Kind::Message(any) => {
            let name = any.type_url.strip_prefix(TYPE_URL).unwrap_or(&any.type_url);
            let descriptor = messages
                .get_message_by_name(name)
                .ok_or_else(|| format!("{name} is not a message of the modules reached"))?;
            Expr::Message(
                DynamicMessage::decode(descriptor, any.value.as_slice())
                    .map_err(|e| format!("a {name} does not read: {e}"))?,
            )
        }
    })
}

/// `value`, as a workflow keeps it.
///
/// # Errors
/// An item of a list or a map that is no value here; why.
pub fn write(value: &Expr) -> Result<Value, String> {
    let kind = match value {
        Expr::Null => Kind::Null(Null::Value.into()),
        Expr::Bool(b) => Kind::Bool(*b),
        Expr::Int(i) => Kind::Int(*i),
        Expr::Uint(u) => Kind::Uint(*u),
        Expr::Double(d) => Kind::Double(*d),
        Expr::String(s) => Kind::String(s.to_string()),
        Expr::Bytes(b) => Kind::Bytes(b.to_vec()),
        Expr::Timestamp(t) => Kind::Timestamp(timestamp(*t)),
        Expr::Duration(d) => Kind::Duration(prost_types::Duration {
            seconds: d.num_seconds(),
            nanos: d.subsec_nanos(),
        }),
        Expr::List(items) => Kind::List(List {
            items: items.iter().map(|i| write(&i?)).collect::<Result<_, _>>()?,
        }),
        Expr::Map(entries) => Kind::Map(Map {
            entries: entries
                .iter()
                .map(|e| {
                    let (k, v) = e?;
                    Ok((k.to_string(), write(&v)?))
                })
                .collect::<Result<_, String>>()?,
        }),
        Expr::Message(m) => {
            let own = OWN.as_ref().map_err(Clone::clone)?;
            if own
                .get_message_by_name(m.descriptor().full_name())
                .is_none()
            {
                return write(&Expr::fields_of(m)?);
            }
            Kind::Message(prost_types::Any {
                type_url: format!("{TYPE_URL}{}", m.descriptor().full_name()),
                value: m.encode_to_vec(),
            })
        }
    };
    Ok(Value { kind: Some(kind) })
}

/// `values`, named, as a workflow keeps them: a map.
///
/// # Errors
/// As [`write`]'s.
pub fn write_map(values: &BTreeMap<String, Expr>) -> Result<Map, String> {
    Ok(Map {
        entries: values
            .iter()
            .map(|(k, v)| Ok((k.clone(), write(v)?)))
            .collect::<Result<_, String>>()?,
    })
}

/// `message`, one of the workflow's own, read by `messages` as CEL reads
/// it.
///
/// # Errors
/// `messages` does not hold its type, or it does not read.
pub fn as_message<T: Message + Name>(
    message: &T,
    messages: &DescriptorPool,
) -> Result<DynamicMessage, String> {
    as_named(message, &T::full_name(), messages)
}

/// `message`, of proto message `name`, read by `messages` as CEL reads it
/// (a message of a package with no type names, the module API's).
///
/// # Errors
/// `messages` does not hold `name`, or it does not read.
pub fn as_named(
    message: &impl Message,
    name: &str,
    messages: &DescriptorPool,
) -> Result<DynamicMessage, String> {
    let descriptor = messages
        .get_message_by_name(name)
        .ok_or_else(|| format!("{name} is not in the descriptors"))?;
    DynamicMessage::decode(descriptor, message.encode_to_vec().as_slice())
        .map_err(|e| format!("a {name} does not read: {e}"))
}
