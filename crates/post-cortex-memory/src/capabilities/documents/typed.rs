// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A document's data as the message its kind declares (W23), read with
//! the module's descriptors: decoded to be checked, redacted, titled and
//! filtered, and encoded again. Data written as JSON before W23 is read
//! into the message with the canonical proto JSON mapping, the one
//! conversion of the migration (export format 1.7.0).

use post_cortex_core::core::document::{DocumentData, KindDeclaration};
use post_cortex_core::redact::{RedactionCounts, Redactor};
use prost::Message;
use prost_reflect::{DynamicMessage, Kind, MessageDescriptor, ReflectMessage, Value as Field};
use serde_json::{Map, Value};

use crate::capabilities::documents::DocumentError;

/// The message of a kind's data.
pub struct Typed {
    message: MessageDescriptor,
    title_field: String,
}

impl Typed {
    /// The message `declared` names in its module's descriptors.
    ///
    /// # Errors
    /// `Kinds` when the descriptors do not read or lack the message (the
    /// host checked both when it loaded the module).
    pub fn of(declared: &KindDeclaration) -> Result<Self, DocumentError> {
        let unreadable = |reason: String| {
            DocumentError::Kinds(post_cortex_ports::KindsError::Invalid(format!(
                "the type of document kind {}: {reason}",
                declared.kind
            )))
        };
        let pool = pcx_module_api::descriptors(&declared.descriptors).map_err(unreadable)?;
        let message = pool
            .get_message_by_name(&declared.type_name)
            .ok_or_else(|| unreadable(format!("no message {}", declared.type_name)))?;
        Ok(Self {
            message,
            title_field: declared.title_field.clone(),
        })
    }

    /// `data` as the kind's message.
    ///
    /// # Errors
    /// `InvalidArgument` when it is another type, or does not decode or
    /// read as one.
    pub fn decode(&self, data: &DocumentData) -> Result<DynamicMessage, DocumentError> {
        let name = self.message.full_name();
        match data {
            DocumentData::Message { type_name, bytes } => {
                if type_name != name {
                    return Err(invalid(format!(
                        "the data is a {type_name}, the kind takes a {name}"
                    )));
                }
                DynamicMessage::decode(self.message.clone(), bytes.as_slice())
                    .map_err(|e| invalid(format!("the data is not a {name}: {e}")))
            }
            DocumentData::Json(map) => {
                let mut map = map.clone();
                as_written_before_w23(&self.message, &mut map);
                DynamicMessage::deserialize(self.message.clone(), Value::Object(map))
                    .map_err(|e| invalid(format!("the data does not read as a {name}: {e}")))
            }
        }
    }

    /// The descriptor of the kind's message.
    pub fn descriptor(&self) -> &MessageDescriptor {
        &self.message
    }

    /// The title of `message`.
    pub fn title(&self, message: &DynamicMessage) -> String {
        match message.get_field_by_name(&self.title_field).as_deref() {
            Some(Field::String(title)) => title.clone(),
            _ => String::new(),
        }
    }
}

/// `message`, encoded.
pub fn encode(message: &DynamicMessage) -> DocumentData {
    DocumentData::Message {
        type_name: message.descriptor().full_name().to_string(),
        bytes: message.encode_to_vec(),
    }
}

/// `message` as JSON, by the canonical proto mapping with its field
/// names, for the MCP tool.
///
/// # Errors
/// `InvalidArgument` for a value JSON cannot hold.
pub fn json(message: &DynamicMessage) -> Result<Map<String, Value>, DocumentError> {
    let options = prost_reflect::SerializeOptions::new()
        .use_proto_field_name(true)
        .skip_default_fields(false);
    match message.serialize_with_options(serde_json::value::Serializer, &options) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(other) => Err(invalid(format!("the data maps to {other}, not an object"))),
        Err(e) => Err(invalid(format!("the data does not map to JSON: {e}"))),
    }
}

/// Every string of `message`, at any depth, redacted.
pub fn redact(message: &mut DynamicMessage, redactor: &Redactor, counts: &mut RedactionCounts) {
    for (_, value) in message.fields_mut() {
        redact_value(value, redactor, counts);
    }
}

fn redact_value(value: &mut Field, redactor: &Redactor, counts: &mut RedactionCounts) {
    match value {
        Field::String(text) => *text = redactor.redact_str(text, counts),
        Field::Message(message) => redact(message, redactor, counts),
        Field::List(items) => {
            for item in items {
                redact_value(item, redactor, counts);
            }
        }
        Field::Map(entries) => {
            for entry in entries.values_mut() {
                redact_value(entry, redactor, counts);
            }
        }
        _ => {}
    }
}

/// `data`, JSON of `message` as modules wrote it before W23, in the proto
/// JSON mapping (at any depth): a `google.protobuf.Struct` field written as
/// its JSON text (the chief's actions) is read as the object it is, and an
/// enum value written as its lowercase name without the enum's prefix
/// (`decide`, a team member's authority) as the value it names.
fn as_written_before_w23(message: &MessageDescriptor, data: &mut Map<String, Value>) {
    for field in message.fields() {
        let Some(value) = data.get_mut(field.name()) else {
            continue;
        };
        let items: Vec<&mut Value> = match value {
            Value::Array(items) => items.iter_mut().collect(),
            value => vec![value],
        };
        for item in items {
            match field.kind() {
                Kind::Message(inner) if inner.full_name() == "google.protobuf.Struct" => {
                    if let Value::String(text) = item
                        && let Ok(object @ Value::Object(_)) = serde_json::from_str::<Value>(text)
                    {
                        *item = object;
                    }
                }
                Kind::Message(inner) => {
                    if let Value::Object(object) = item {
                        as_written_before_w23(&inner, object);
                    }
                }
                Kind::Enum(values) => {
                    if let Value::String(name) = item
                        && values.get_value_by_name(name).is_none()
                    {
                        let suffix = format!("_{}", name.to_uppercase());
                        let named: Vec<_> = values
                            .values()
                            .filter(|v| v.name().ends_with(&suffix))
                            .collect();
                        if let [one] = named.as_slice() {
                            *item = Value::String(one.name().to_string());
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

fn invalid(reason: String) -> DocumentError {
    DocumentError::InvalidArgument(reason)
}
