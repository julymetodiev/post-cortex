// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Whether a view's request templates and pointers fit the messages of the
//! RPCs they name (W23): the methods a manifest names, from its
//! descriptors; a template's keys are fields of the request, a pointer's
//! tokens fields of the response, as the proto JSON mapping writes them
//! (proto field names).

use std::collections::HashMap;

use prost_reflect::{FieldDescriptor, Kind, MessageDescriptor, MethodDescriptor};
use serde_json::{Map, Value};

use crate::Manifest;

/// The methods of the RPCs `manifest` names, by their gRPC path; none
/// without views.
///
/// # Errors
/// Its descriptors do not read, or an RPC is not one of their methods.
pub fn methods(manifest: &Manifest) -> Result<HashMap<&str, MethodDescriptor>, String> {
    if manifest.views.is_empty() || manifest.rpcs.is_empty() {
        return Ok(HashMap::new());
    }
    let pool = crate::descriptors(&manifest.descriptors)
        .map_err(|e| format!("of a module whose descriptors do not read: {e}"))?;
    let mut methods = HashMap::new();
    for rpc in &manifest.rpcs {
        let method = rpc
            .method
            .strip_prefix('/')
            .and_then(|path| path.split_once('/'))
            .and_then(|(service, name)| {
                pool.get_service_by_name(service)?
                    .methods()
                    .find(|m| m.name() == name)
            })
            .ok_or_else(|| {
                format!(
                    "of a module whose rpc {} is not a method of its descriptors",
                    rpc.method
                )
            })?;
        methods.insert(rpc.method.as_str(), method);
    }
    Ok(methods)
}

/// Messages whose JSON form is not an object of their fields.
fn opaque(message: &MessageDescriptor) -> bool {
    message.package_name() == "google.protobuf"
}

/// `template` is a request of `message`: its keys its fields, an object
/// under a message field that message's.
///
/// # Errors
/// The first key that is not a field, by its path.
pub fn request(message: &MessageDescriptor, template: &Map<String, Value>) -> Result<(), String> {
    for (key, value) in template {
        let field = message
            .get_field_by_name(key)
            .ok_or_else(|| format!("{key} is not a field of {}", message.full_name()))?;
        let inner = match (field.kind(), field.is_map()) {
            (Kind::Message(inner), false) if !opaque(&inner) => inner,
            _ => continue,
        };
        let objects: Vec<&Map<String, Value>> = match value {
            Value::Object(object) => vec![object],
            Value::Array(items) => items.iter().filter_map(Value::as_object).collect(),
            _ => Vec::new(),
        };
        for object in objects {
            request(&inner, object).map_err(|e| format!("{key}.{e}"))?;
        }
    }
    Ok(())
}

/// What a pointer has reached so far.
enum At {
    /// A message: the next token is one of its fields.
    Message(MessageDescriptor),
    /// A list or a map: the next token is an index or a key, then its
    /// element.
    Elements(FieldDescriptor),
    /// What the JSON mapping leaves open (`Struct`, `Value`, `Any`): any
    /// token.
    Open,
    /// A scalar: no token.
    Leaf,
}

/// What a value of `kind` is to a pointer.
fn at_kind(kind: Kind) -> At {
    match kind {
        Kind::Message(message) if opaque(&message) => At::Open,
        Kind::Message(message) => At::Message(message),
        _ => At::Leaf,
    }
}

/// `pointer` is a JSON Pointer (RFC 6901) into a response of `message`.
///
/// # Errors
/// It is not a JSON Pointer, or a token names no field.
pub fn pointer(message: &MessageDescriptor, pointer: &str) -> Result<(), String> {
    if pointer.is_empty() {
        return Ok(());
    }
    let tokens = pointer
        .strip_prefix('/')
        .ok_or_else(|| format!("{pointer} is not a JSON Pointer"))?;
    let mut at = At::Message(message.clone());
    for raw in tokens.split('/') {
        let token = unescape(raw).ok_or_else(|| format!("{pointer} is not a JSON Pointer"))?;
        at = match at {
            At::Message(message) => {
                let field = message.get_field_by_name(&token).ok_or_else(|| {
                    format!(
                        "{pointer}: {token} is not a field of {}",
                        message.full_name()
                    )
                })?;
                if field.is_list() || field.is_map() {
                    At::Elements(field)
                } else {
                    at_kind(field.kind())
                }
            }
            At::Elements(field) if field.is_map() => match field.kind() {
                Kind::Message(entry) => at_kind(entry.map_entry_value_field().kind()),
                _ => At::Leaf,
            },
            At::Elements(field) => {
                token
                    .parse::<usize>()
                    .map_err(|_| format!("{pointer}: {token} is not an index"))?;
                at_kind(field.kind())
            }
            At::Open => At::Open,
            At::Leaf => return Err(format!("{pointer}: {token} is under a scalar")),
        };
    }
    Ok(())
}

/// A pointer token with `~1` as `/` and `~0` as `~`; none when another `~`
/// is in it.
fn unescape(token: &str) -> Option<String> {
    let mut out = String::with_capacity(token.len());
    let mut chars = token.chars();
    while let Some(c) = chars.next() {
        match (c, c == '~') {
            (c, false) => out.push(c),
            (_, true) => out.push(match chars.next() {
                Some('0') => '~',
                Some('1') => '/',
                _ => return None,
            }),
        }
    }
    Some(out)
}
