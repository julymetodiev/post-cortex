// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A `Template` filled when its block runs (docs/modules/workflow.md,
//! templates): a request message built field by field from its constants
//! and its expressions' values, or a called workflow's input by its
//! schema's fields. The check has typed every slot; what fails here is a
//! value out of a field's range or an expression that fails.

use std::collections::{BTreeMap, HashMap};

use pcx_expr::{Value as Expr, Values};
use post_cortex_proto::workflow::{Slot, Template, slot};
use prost_reflect::{
    DescriptorPool, DynamicMessage, FieldDescriptor, Kind, MapKey, MessageDescriptor,
    ReflectMessage, Value as Proto,
};

use crate::domain::slots::Texts;
use crate::domain::value::read;
use pcx_expr::schema::Field;

/// What went wrong filling a field: its path and why.
pub type Unfilled = (String, String);

/// What a template is filled with: its texts compiled when it was
/// checked, the messages its constants read by, and the values on the
/// block's path.
pub struct With<'a> {
    /// Its texts, by the path of the field each fills.
    pub texts: &'a Texts,
    /// The messages of the run's catalog.
    pub messages: &'a DescriptorPool,
    /// The values its texts read.
    pub values: &'a Values,
}

/// `template` filled as a `message`, the fields under `under` (the path of
/// the message, empty at the top).
///
/// # Errors
/// The field at fault and why.
pub fn message(
    template: &Template,
    message: &MessageDescriptor,
    with: &With<'_>,
    under: &str,
) -> Result<DynamicMessage, Unfilled> {
    let mut built = DynamicMessage::new(message.clone());
    for (name, slot) in &template.fields {
        let at = if under.is_empty() {
            name.clone()
        } else {
            format!("{under}.{name}")
        };
        let field = message.get_field_by_name(name).ok_or_else(|| {
            (
                at.clone(),
                format!("is no field of {}", message.full_name()),
            )
        })?;
        let value = match slot.slot.as_ref() {
            Some(slot::Slot::Message(nested)) => match field.kind() {
                Kind::Message(m) => Proto::Message(self::message(nested, &m, with, &at)?),
                _ => return Err((at, "is not a message field".into())),
            },
            Some(slot::Slot::List(list)) => Proto::List(
                list.items
                    .iter()
                    .enumerate()
                    .map(|(i, item)| item_value(item, &field, with, &format!("{at}[{i}]")))
                    .collect::<Result<_, _>>()?,
            ),
            _ => {
                let value = slot_value(slot, with, &at)?;
                if value == Expr::Null {
                    continue;
                }
                field_value(&field, value).map_err(|why| (at.clone(), why))?
            }
        };
        built
            .try_set_field(&field, value)
            .map_err(|e| (at.clone(), e.to_string()))?;
    }
    Ok(built)
}

/// `template` filled as a called workflow's input: each field of
/// `fields` given or its zero value.
///
/// # Errors
/// The field at fault and why.
pub fn input(
    template: &Template,
    fields: &[Field],
    with: &With<'_>,
) -> Result<BTreeMap<String, Expr>, Unfilled> {
    let mut given = BTreeMap::new();
    for field in fields {
        let value = match template.fields.get(&field.name) {
            Some(slot) => match slot.slot.as_ref() {
                Some(slot::Slot::List(list)) => Expr::List(
                    list.items
                        .iter()
                        .enumerate()
                        .map(|(i, s)| slot_value(s, with, &format!("{}[{i}]", field.name)))
                        .collect::<Result<_, _>>()?,
                ),
                _ => slot_value(slot, with, &field.name)?,
            },
            None => pcx_expr::schema::zero(&field.ty),
        };
        given.insert(field.name.clone(), value);
    }
    Ok(given)
}

/// A constant's or a text's value, the field at `at`.
fn slot_value(slot: &Slot, with: &With<'_>, at: &str) -> Result<Expr, Unfilled> {
    let refuse = |why: String| (at.to_string(), why);
    match slot.slot.as_ref() {
        Some(slot::Slot::Constant(value)) => read(value, with.messages).map_err(refuse),
        Some(slot::Slot::Text(_)) => with
            .texts
            .get(at)
            .ok_or_else(|| refuse("its text was not checked".into()))?
            .value(with.values)
            .map_err(|e| refuse(e.to_string())),
        Some(slot::Slot::Message(_) | slot::Slot::List(_)) | None => {
            Err(refuse("is not a value here".into()))
        }
    }
}

/// One item of a repeated `field`.
fn item_value(
    slot: &Slot,
    field: &FieldDescriptor,
    with: &With<'_>,
    at: &str,
) -> Result<Proto, Unfilled> {
    match (slot.slot.as_ref(), field.kind()) {
        (Some(slot::Slot::Message(nested)), Kind::Message(m)) => {
            Ok(Proto::Message(message(nested, &m, with, at)?))
        }
        (_, kind) => {
            let value = slot_value(slot, with, at)?;
            kind_value(&kind, value).map_err(|why| (at.to_string(), why))
        }
    }
}

/// `value` as proto field `field` holds it: a list, a map or one value.
pub fn field_value(field: &FieldDescriptor, value: Expr) -> Result<Proto, String> {
    if field.is_map() {
        let Kind::Message(entry) = field.kind() else {
            return Err("a map field without its entry".into());
        };
        let Expr::Map(entries) = value else {
            return Err("is not a map".into());
        };
        let kind = entry.map_entry_value_field().kind();
        return Ok(Proto::Map(
            entries
                .iter()
                .map(|e| {
                    let (k, v) = e?;
                    Ok((MapKey::String(k.to_string()), kind_value(&kind, v)?))
                })
                .collect::<Result<HashMap<_, _>, String>>()?,
        ));
    }
    if field.is_list() {
        let Expr::List(items) = value else {
            return Err("is not a list".into());
        };
        let kind = field.kind();
        return Ok(Proto::List(
            items
                .iter()
                .map(|v| kind_value(&kind, v?))
                .collect::<Result<_, _>>()?,
        ));
    }
    kind_value(&field.kind(), value)
}

/// `value` as a proto value of `kind`.
fn kind_value(kind: &Kind, value: Expr) -> Result<Proto, String> {
    let range = |what: &str| format!("is out of the range of {what}");
    Ok(match (kind, value) {
        (Kind::Bool, Expr::Bool(b)) => Proto::Bool(b),
        (Kind::String, Expr::String(s)) => Proto::String(s.to_string()),
        (Kind::Bytes, Expr::Bytes(b)) => Proto::Bytes(b.to_vec().into()),
        (Kind::Double, Expr::Double(d)) => Proto::F64(d),
        (Kind::Float, Expr::Double(d)) => Proto::F32(d as f32),
        (Kind::Int64 | Kind::Sint64 | Kind::Sfixed64, Expr::Int(i)) => Proto::I64(i),
        (Kind::Int32 | Kind::Sint32 | Kind::Sfixed32, Expr::Int(i)) => {
            Proto::I32(i32::try_from(i).map_err(|_| range("an int32"))?)
        }
        (Kind::Uint64 | Kind::Fixed64, Expr::Uint(u)) => Proto::U64(u),
        (Kind::Uint32 | Kind::Fixed32, Expr::Uint(u)) => {
            Proto::U32(u32::try_from(u).map_err(|_| range("a uint32"))?)
        }
        (Kind::Enum(e), Expr::Int(i)) => {
            let number = i32::try_from(i).map_err(|_| range("an enum"))?;
            if e.get_value(number).is_none() {
                return Err(format!("{number} is no value of {}", e.full_name()));
            }
            Proto::EnumNumber(number)
        }
        (Kind::Message(m), value) => Proto::Message(well_known(m, value)?),
        (kind, value) => {
            return Err(format!(
                "a {} is not a {}",
                value.ty(),
                pcx_expr::Type::of_kind(kind)
            ));
        }
    })
}

/// `value` as message `m`: itself when it is one, else a well-known type
/// (a time, a duration, a wrapper, a JSON value or struct, an `Any`).
fn well_known(m: &MessageDescriptor, value: Expr) -> Result<DynamicMessage, String> {
    let mut built = DynamicMessage::new(m.clone());
    let set = |built: &mut DynamicMessage, name: &str, v: Proto| {
        built
            .try_set_field_by_name(name, v)
            .map_err(|e| e.to_string())
    };
    match (m.full_name(), value) {
        (_, Expr::Message(given)) if given.descriptor().full_name() == m.full_name() => {
            return Ok(given);
        }
        // Another module's message, kept by its fields.
        (name, value @ Expr::Map(_)) if !name.starts_with("google.protobuf.") => {
            let json = value.json()?;
            return DynamicMessage::deserialize(m.clone(), json).map_err(|e| e.to_string());
        }
        ("google.protobuf.Any", Expr::Message(given)) => {
            set(
                &mut built,
                "type_url",
                Proto::String(format!(
                    "type.googleapis.com/{}",
                    given.descriptor().full_name()
                )),
            )?;
            set(
                &mut built,
                "value",
                Proto::Bytes(prost::Message::encode_to_vec(&given).into()),
            )?;
        }
        ("google.protobuf.Timestamp", Expr::Timestamp(t)) => {
            set(&mut built, "seconds", Proto::I64(t.timestamp()))?;
            // Below 2 * 10^9 (a leap second): an i32 holds it.
            set(
                &mut built,
                "nanos",
                Proto::I32(t.timestamp_subsec_nanos() as i32),
            )?;
        }
        ("google.protobuf.Duration", Expr::Duration(d)) => {
            set(&mut built, "seconds", Proto::I64(d.num_seconds()))?;
            set(&mut built, "nanos", Proto::I32(d.subsec_nanos()))?;
        }
        (
            "google.protobuf.Value" | "google.protobuf.Struct" | "google.protobuf.ListValue",
            value,
        ) => {
            return DynamicMessage::deserialize(m.clone(), value.json()?)
                .map_err(|e| format!("does not read as a {}: {e}", m.full_name()));
        }
        (name, value) if name.starts_with("google.protobuf.") && name.ends_with("Value") => {
            let field = m
                .get_field_by_name("value")
                .ok_or_else(|| format!("{name} has no value"))?;
            set(&mut built, "value", kind_value(&field.kind(), value)?)?;
        }
        (name, value) => return Err(format!("a {} is not a {name}", value.ty())),
    }
    Ok(built)
}
