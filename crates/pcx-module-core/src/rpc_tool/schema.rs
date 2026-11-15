// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What agents read of an RPC as a tool, from the module's descriptors:
//! its comment, and the JSON Schema of its request message as the
//! canonical proto JSON mapping reads it (proto field names). A message
//! it holds in more than one field, or that holds itself, is written once,
//! in `$defs`; one held once is written where it is.

use std::collections::{HashMap, HashSet};

use prost_reflect::{Cardinality, FieldDescriptor, Kind, MessageDescriptor, MethodDescriptor};
use serde_json::{Map, Value, json};

/// How deep a call's JSON is followed to name a field it does not have.
const MAX_DEPTH: usize = 8;

/// The leading comment of the declaration at `path` of `file`.
fn comment_at(file: &prost_reflect::FileDescriptor, path: &[i32]) -> String {
    file.file_descriptor_proto()
        .source_code_info
        .as_ref()
        .and_then(|info| info.location.iter().find(|l| l.path == path))
        .and_then(|l| l.leading_comments.as_deref())
        .map(|c| c.split_whitespace().collect::<Vec<_>>().join(" "))
        .unwrap_or_default()
}

/// The comment of `method`: its tool's description.
pub fn comments(method: &MethodDescriptor) -> String {
    comment_at(&method.parent_file(), method.path())
}

/// The JSON Schema of `message`, an object of its fields, with the
/// messages it shares in `$defs`, by full name.
pub fn object(message: &MessageDescriptor) -> Map<String, Value> {
    let mut held = Held::default();
    held.visit(message, &mut Vec::new());
    let mut defs = Defs {
        root: message.full_name().to_string(),
        held,
        written: Map::new(),
    };
    let mut schema = defs.fields(message);
    if !defs.written.is_empty() {
        schema.insert("$defs".to_string(), Value::Object(defs.written));
    }
    schema
}

/// The schema of a well-known type, which is not written as its fields.
fn well_known(name: &str) -> Option<Value> {
    match name {
        "google.protobuf.Timestamp" => Some(json!({ "type": "string", "format": "date-time" })),
        "google.protobuf.Duration" => Some(json!({ "type": "string" })),
        "google.protobuf.Struct" | "google.protobuf.Any" => Some(json!({ "type": "object" })),
        "google.protobuf.Value" => Some(json!({})),
        "google.protobuf.ListValue" => Some(json!({ "type": "array" })),
        _ => None,
    }
}

/// The message a field holds (a map's: its value's), if it holds one that
/// is written as its fields.
fn held_message(field: &FieldDescriptor) -> Option<MessageDescriptor> {
    let kind = match field.kind() {
        Kind::Message(entry) if field.is_map() => entry.map_entry_value_field().kind(),
        other => other,
    };
    match kind {
        Kind::Message(message) if well_known(message.full_name()).is_none() => Some(message),
        _ => None,
    }
}

/// How the messages under a schema's are held: by how many fields (each
/// message's fields counted once), and which hold themselves.
#[derive(Default)]
struct Held {
    uses: HashMap<String, usize>,
    cyclic: HashSet<String>,
    seen: HashSet<String>,
}

impl Held {
    fn visit(&mut self, message: &MessageDescriptor, path: &mut Vec<String>) {
        let name = message.full_name().to_string();
        if let Some(at) = path.iter().position(|open| *open == name) {
            self.cyclic.extend(path[at..].iter().cloned());
            return;
        }
        if !self.seen.insert(name.clone()) {
            return;
        }
        path.push(name);
        for field in message.fields() {
            if let Some(inner) = held_message(&field) {
                *self.uses.entry(inner.full_name().to_string()).or_default() += 1;
                self.visit(&inner, path);
            }
        }
        path.pop();
    }

    /// Whether `name` is written once in `$defs` rather than where it is.
    fn shared(&self, name: &str) -> bool {
        self.cyclic.contains(name) || self.uses.get(name).is_some_and(|n| *n > 1)
    }
}

/// The schema being written: the shared messages each once by full name,
/// named by `$ref` where a field holds them (`#` for the schema's own
/// message). A `$ref` keeps `"type": "object"` beside it, for a reader
/// that looks at the type alone.
struct Defs {
    root: String,
    held: Held,
    written: Map<String, Value>,
}

impl Defs {
    /// The object of `message`'s fields.
    fn fields(&mut self, message: &MessageDescriptor) -> Map<String, Value> {
        let file = message.parent_file();
        let properties: Map<String, Value> = message
            .fields()
            .map(|field| {
                let mut schema = self.field(&field);
                let description = comment_at(&file, field.path());
                if let (Value::Object(map), false) = (&mut schema, description.is_empty()) {
                    map.insert("description".to_string(), Value::from(description));
                }
                (field.name().to_string(), schema)
            })
            .collect();
        let mut schema = Map::new();
        schema.insert("type".to_string(), Value::from("object"));
        schema.insert("properties".to_string(), Value::Object(properties));
        schema
    }

    /// `message` where a field holds it: a well-known type as itself, one
    /// held once as its fields, a shared one by `$ref`, written in `$defs`
    /// the first time.
    fn message(&mut self, message: &MessageDescriptor) -> Value {
        let name = message.full_name();
        if let Some(schema) = well_known(name) {
            return schema;
        }
        if name == self.root {
            return json!({ "type": "object", "$ref": "#" });
        }
        if !self.held.shared(name) {
            return Value::Object(self.fields(message));
        }
        if !self.written.contains_key(name) {
            // Taken before its fields are read, so a message that holds
            // itself is named instead of written again.
            self.written.insert(name.to_string(), Value::Null);
            let schema = self.fields(message);
            self.written.insert(name.to_string(), Value::Object(schema));
        }
        json!({ "type": "object", "$ref": format!("#/$defs/{name}") })
    }

    fn field(&mut self, field: &FieldDescriptor) -> Value {
        if field.is_map() {
            let value = match field.kind() {
                Kind::Message(entry) => entry.map_entry_value_field().kind().clone(),
                other => other,
            };
            return json!({ "type": "object", "additionalProperties": self.kind(&value) });
        }
        let one = self.kind(&field.kind());
        if field.cardinality() == Cardinality::Repeated {
            json!({ "type": "array", "items": one })
        } else {
            one
        }
    }

    fn kind(&mut self, kind: &Kind) -> Value {
        match kind {
            Kind::Double | Kind::Float => json!({ "type": "number" }),
            Kind::Int32
            | Kind::Sint32
            | Kind::Sfixed32
            | Kind::Int64
            | Kind::Sint64
            | Kind::Sfixed64 => json!({ "type": "integer" }),
            Kind::Uint32 | Kind::Fixed32 | Kind::Uint64 | Kind::Fixed64 => {
                json!({ "type": "integer", "minimum": 0 })
            }
            Kind::Bool => json!({ "type": "boolean" }),
            Kind::String => json!({ "type": "string" }),
            Kind::Bytes => json!({ "type": "string", "contentEncoding": "base64" }),
            Kind::Enum(values) => {
                let names: Vec<String> = values.values().map(|v| v.name().to_string()).collect();
                json!({ "type": "string", "enum": names })
            }
            Kind::Message(message) => self.message(message),
        }
    }
}

#[cfg(test)]
mod tests;

/// Where `input` names a field its message does not have, for an agent to
/// see where its call went wrong: the path of that key, the message there
/// and the fields it has. A well-known type's JSON (a `Struct`'s keys) is
/// not its fields, and a map's keys are its own.
pub fn unknown_field(message: &MessageDescriptor, input: &Value) -> Option<String> {
    unknown_at(message, input, "", 0)
}

fn unknown_at(
    message: &MessageDescriptor,
    input: &Value,
    path: &str,
    depth: usize,
) -> Option<String> {
    let Value::Object(map) = input else {
        return None;
    };
    if depth >= MAX_DEPTH || message.full_name().starts_with("google.protobuf.") {
        return None;
    }
    map.iter().find_map(|(key, value)| {
        let at = if path.is_empty() {
            key.clone()
        } else {
            format!("{path}.{key}")
        };
        let Some(field) = message
            .get_field_by_name(key)
            .or_else(|| message.get_field_by_json_name(key))
        else {
            let fields: Vec<String> = message.fields().map(|f| f.name().to_string()).collect();
            return Some(format!(
                "{at}: {} has no field {key} (its fields: {})",
                message.full_name(),
                fields.join(", ")
            ));
        };
        let Kind::Message(inner) = field.kind() else {
            return None;
        };
        match (field.is_map(), field.cardinality(), value) {
            (true, _, _) => None,
            (false, Cardinality::Repeated, Value::Array(items)) => items
                .iter()
                .enumerate()
                .find_map(|(i, item)| unknown_at(&inner, item, &format!("{at}[{i}]"), depth + 1)),
            _ => unknown_at(&inner, value, &at, depth + 1),
        }
    })
}
