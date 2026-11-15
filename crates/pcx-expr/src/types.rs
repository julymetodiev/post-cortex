// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The types of values and expressions: as an input or a schema's field
//! declares them (CEL's text), as a proto field has them, and as CEL checks
//! them, converted in this one place.

use std::fmt;
use std::sync::Arc;

use cel_core::CelType;
use prost_reflect::{FieldDescriptor, Kind};

/// A type, as CEL writes it: `string`, `int`, `uint`, `double`, `bool`,
/// `bytes`, `timestamp`, `duration`, `list<T>`, `map<string, T>`, a proto
/// message by its full name; `null` and `dyn` only as CEL infers them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Type {
    /// Text.
    String,
    /// A signed whole number.
    Int,
    /// An unsigned whole number.
    Uint,
    /// A number.
    Double,
    /// True or false.
    Bool,
    /// Bytes.
    Bytes,
    /// A time.
    Timestamp,
    /// A duration.
    Duration,
    /// A list of one type.
    List(Box<Type>),
    /// A map from text to one type.
    Map(Box<Type>),
    /// A proto message, by its full name.
    Message(String),
    /// The null value.
    Null,
    /// Known only when it runs.
    Dyn,
}

impl Type {
    /// The type `text` writes: one an input or a schema's field may
    /// declare (no message, `null` or `dyn`).
    ///
    /// # Errors
    /// It writes none of them; the reason.
    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        let inner = |prefix: &str| {
            text.strip_prefix(prefix)
                .and_then(|rest| rest.strip_suffix('>'))
        };
        match text {
            "string" => Ok(Self::String),
            "int" => Ok(Self::Int),
            "uint" => Ok(Self::Uint),
            "double" => Ok(Self::Double),
            "bool" => Ok(Self::Bool),
            "bytes" => Ok(Self::Bytes),
            "timestamp" => Ok(Self::Timestamp),
            "duration" => Ok(Self::Duration),
            _ => {
                if let Some(item) = inner("list<") {
                    return Ok(Self::List(Box::new(Self::parse(item)?)));
                }
                if let Some(entry) = inner("map<") {
                    let value = entry
                        .trim_start()
                        .strip_prefix("string")
                        .and_then(|rest| rest.trim_start().strip_prefix(','))
                        .ok_or_else(|| format!("\"{text}\": a map's keys are strings"))?;
                    return Ok(Self::Map(Box::new(Self::parse(value)?)));
                }
                Err(format!(
                    "\"{text}\" is no type (string, int, uint, double, bool, bytes, timestamp, duration, list<T>, map<string, T>)"
                ))
            }
        }
    }

    /// The type of proto field `field`: a repeated field a list, a map
    /// field a map (its keys text), an enum an int, the well-known
    /// messages CEL's own types.
    ///
    /// # Errors
    /// A map whose keys are not text; the reason.
    pub fn of_field(field: &FieldDescriptor) -> Result<Self, String> {
        if field.is_map() {
            let Kind::Message(entry) = field.kind() else {
                return Err(format!(
                    "{}: a map field without its entry",
                    field.full_name()
                ));
            };
            let key = entry.map_entry_key_field();
            if key.kind() != Kind::String {
                return Err(format!("{}: a map's keys are not text", field.full_name()));
            }
            return Ok(Self::Map(Box::new(Self::of_kind(
                &entry.map_entry_value_field().kind(),
            ))));
        }
        let item = Self::of_kind(&field.kind());
        Ok(if field.is_list() {
            Self::List(Box::new(item))
        } else {
            item
        })
    }

    /// The type of a proto value of `kind`.
    pub fn of_kind(kind: &Kind) -> Self {
        match kind {
            Kind::Bool => Self::Bool,
            Kind::Int32
            | Kind::Sint32
            | Kind::Sfixed32
            | Kind::Int64
            | Kind::Sint64
            | Kind::Sfixed64
            | Kind::Enum(_) => Self::Int,
            Kind::Uint32 | Kind::Fixed32 | Kind::Uint64 | Kind::Fixed64 => Self::Uint,
            Kind::Float | Kind::Double => Self::Double,
            Kind::String => Self::String,
            Kind::Bytes => Self::Bytes,
            Kind::Message(message) => Self::of_message(message.full_name()),
        }
    }

    /// The type of proto message `name`: CEL's own for the well-known
    /// types.
    pub fn of_message(name: &str) -> Self {
        match name {
            "google.protobuf.Timestamp" => Self::Timestamp,
            "google.protobuf.Duration" => Self::Duration,
            "google.protobuf.StringValue" => Self::String,
            "google.protobuf.BoolValue" => Self::Bool,
            "google.protobuf.Int32Value" | "google.protobuf.Int64Value" => Self::Int,
            "google.protobuf.UInt32Value" | "google.protobuf.UInt64Value" => Self::Uint,
            "google.protobuf.FloatValue" | "google.protobuf.DoubleValue" => Self::Double,
            "google.protobuf.BytesValue" => Self::Bytes,
            "google.protobuf.Any" | "google.protobuf.Value" => Self::Dyn,
            "google.protobuf.Struct" => Self::Map(Box::new(Self::Dyn)),
            "google.protobuf.ListValue" => Self::List(Box::new(Self::Dyn)),
            name => Self::Message(name.to_string()),
        }
    }

    /// The type CEL checked an expression to; none for one no value of
    /// this crate has (an optional, a type, a function, a map not from
    /// text).
    pub fn of_cel(ty: &CelType) -> Option<Self> {
        Some(match ty {
            CelType::Bool => Self::Bool,
            CelType::Int | CelType::Enum(_) => Self::Int,
            CelType::UInt => Self::Uint,
            CelType::Double => Self::Double,
            CelType::String => Self::String,
            CelType::Bytes => Self::Bytes,
            CelType::Timestamp => Self::Timestamp,
            CelType::Duration => Self::Duration,
            CelType::Null => Self::Null,
            CelType::Dyn => Self::Dyn,
            CelType::List(item) => Self::List(Box::new(Self::of_cel(item)?)),
            CelType::Map(key, value) => match **key {
                CelType::String | CelType::Dyn => Self::Map(Box::new(Self::of_cel(value)?)),
                _ => return None,
            },
            CelType::Message(name) => Self::of_message(name),
            _ => return None,
        })
    }

    /// The type as CEL checks it.
    pub fn cel(&self) -> CelType {
        match self {
            Self::String => CelType::String,
            Self::Int => CelType::Int,
            Self::Uint => CelType::UInt,
            Self::Double => CelType::Double,
            Self::Bool => CelType::Bool,
            Self::Bytes => CelType::Bytes,
            Self::Timestamp => CelType::Timestamp,
            Self::Duration => CelType::Duration,
            Self::List(item) => CelType::List(Arc::new(item.cel())),
            Self::Map(value) => CelType::Map(Arc::new(CelType::String), Arc::new(value.cel())),
            Self::Message(name) => CelType::Message(Arc::from(name.as_str())),
            Self::Null => CelType::Null,
            Self::Dyn => CelType::Dyn,
        }
    }

    /// Whether a value of type `given` may be put where this type is: the
    /// same type, `dyn` on either side, null where a message is, or lists
    /// and maps of such.
    pub fn accepts(&self, given: &Self) -> bool {
        match (self, given) {
            (Self::Dyn, _) | (_, Self::Dyn) => true,
            (Self::Message(_), Self::Null) => true,
            (Self::List(want), Self::List(have)) | (Self::Map(want), Self::Map(have)) => {
                want.accepts(have)
            }
            (want, have) => want == have,
        }
    }

    /// Whether the whole type is known before it runs: no `dyn` in it
    /// (a value of an expression of such a type needs no check when it
    /// runs).
    #[must_use]
    pub fn known(&self) -> bool {
        match self {
            Self::Dyn => false,
            Self::List(inner) | Self::Map(inner) => inner.known(),
            _ => true,
        }
    }

    /// Whether text shows a value of this type (`dyn`: known only when it
    /// runs).
    pub fn shown(&self) -> bool {
        matches!(
            self,
            Self::String
                | Self::Int
                | Self::Uint
                | Self::Double
                | Self::Bool
                | Self::Timestamp
                | Self::Duration
                | Self::Dyn
        )
    }
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::String => write!(f, "string"),
            Self::Int => write!(f, "int"),
            Self::Uint => write!(f, "uint"),
            Self::Double => write!(f, "double"),
            Self::Bool => write!(f, "bool"),
            Self::Bytes => write!(f, "bytes"),
            Self::Timestamp => write!(f, "timestamp"),
            Self::Duration => write!(f, "duration"),
            Self::List(item) => write!(f, "list<{item}>"),
            Self::Map(value) => write!(f, "map<string, {value}>"),
            Self::Message(name) => write!(f, "{name}"),
            Self::Null => write!(f, "null"),
            Self::Dyn => write!(f, "dyn"),
        }
    }
}

#[cfg(test)]
mod tests;
