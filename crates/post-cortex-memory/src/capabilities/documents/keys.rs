// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The keys of a document (docs/memory.md, document keys and pages): one
//! for every top-level singular scalar field of its message, default
//! values included, so a filter of such fields is a lookup of keys.

use post_cortex_core::core::document::DocumentKey;
use prost_reflect::{
    DynamicMessage, FieldDescriptor, Kind, MessageDescriptor, ReflectMessage, Value,
};
use sha2::{Digest, Sha256};

use crate::capabilities::documents::DocumentError;

/// Whether `field` is a top-level singular scalar: what a key is made of.
fn scalar(field: &FieldDescriptor) -> bool {
    !field.is_list() && !field.is_map() && !matches!(field.kind(), Kind::Message(_))
}

/// The bytes `value` is hashed as.
fn encoded(value: &Value) -> Option<Vec<u8>> {
    Some(match value {
        Value::String(text) => text.as_bytes().to_vec(),
        Value::Bytes(bytes) => bytes.to_vec(),
        Value::Bool(on) => vec![u8::from(*on)],
        Value::I32(n) => i64::from(*n).to_be_bytes().to_vec(),
        Value::I64(n) => n.to_be_bytes().to_vec(),
        Value::U32(n) => u64::from(*n).to_be_bytes().to_vec(),
        Value::U64(n) => n.to_be_bytes().to_vec(),
        Value::F32(n) => u64::from(n.to_bits()).to_be_bytes().to_vec(),
        Value::F64(n) => n.to_bits().to_be_bytes().to_vec(),
        Value::EnumNumber(n) => n.to_be_bytes().to_vec(),
        Value::Message(_) | Value::List(_) | Value::Map(_) => return None,
    })
}

/// The key of `field` with `value`; none for a value that is not scalar.
fn key_of(field: &FieldDescriptor, value: &Value) -> Option<DocumentKey> {
    let digest = Sha256::digest(encoded(value)?);
    let mut first = [0u8; 16];
    first.copy_from_slice(&digest[..16]);
    Some(DocumentKey::new(field.number(), first))
}

/// The keys of `message`: one per top-level singular scalar field.
pub fn of(message: &DynamicMessage) -> Vec<DocumentKey> {
    message
        .descriptor()
        .fields()
        .filter(scalar)
        .filter_map(|field| key_of(&field, &message.get_field(&field)))
        .collect()
}

/// The keys a filter of `fields` equal to `example`'s looks up.
///
/// # Errors
/// `InvalidArgument` for a field the message does not have, or one that
/// is not a top-level singular scalar.
pub fn of_filter(
    descriptor: &MessageDescriptor,
    example: &DynamicMessage,
    fields: &[String],
) -> Result<Vec<DocumentKey>, DocumentError> {
    let mut keys = Vec::with_capacity(fields.len());
    let mut unknown = Vec::new();
    for name in fields {
        let Some(field) = descriptor.get_field_by_name(name) else {
            unknown.push(name.as_str());
            continue;
        };
        let refused = || {
            DocumentError::InvalidArgument(format!(
                "a filter compares top-level singular scalar fields; {} of {} is not one",
                name,
                descriptor.full_name()
            ))
        };
        if !scalar(&field) {
            return Err(refused());
        }
        keys.push(key_of(&field, &example.get_field(&field)).ok_or_else(refused)?);
    }
    if !unknown.is_empty() {
        return Err(DocumentError::InvalidArgument(format!(
            "{} has no field {}",
            descriptor.full_name(),
            unknown.join(", ")
        )));
    }
    Ok(keys)
}
