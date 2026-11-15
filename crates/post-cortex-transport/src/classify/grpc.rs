// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! gRPC calls of `pcx.v1.PostCortex`, `pcx.v1.Blobs` and
//! `pcx.views.v1.Views`, read
//! through their descriptor sets into the JSON form of the message (proto
//! field names, defaults left out).

use std::sync::OnceLock;

use post_cortex_memory::tools::operations::{Effect, OPERATIONS, Operation};
use prost_reflect::{
    DescriptorPool, DynamicMessage, Kind, MessageDescriptor, ReflectMessage, SerializeOptions,
    Value as Field,
};
use serde_json::Value;

use post_cortex_memory::tools::operations::classify::{self as json, of_json};

use crate::classify::{Classified, ClassifyError};

const PCX: &str = "pcx.v1.PostCortex";
const BLOBS: &str = "pcx.v1.Blobs";
const VIEWS: &str = "pcx.views.v1.Views";

/// The descriptors of both services, decoded once.
fn pool() -> Result<&'static DescriptorPool, ClassifyError> {
    static POOL: OnceLock<DescriptorPool> = OnceLock::new();
    if let Some(pool) = POOL.get() {
        return Ok(pool);
    }
    let mut pool = DescriptorPool::new();
    for set in [
        post_cortex_proto::PCX_DESCRIPTOR_SET,
        post_cortex_proto::VIEWS_DESCRIPTOR_SET,
    ] {
        pool.decode_file_descriptor_set(set)
            .map_err(|e| ClassifyError::Malformed(format!("descriptor set: {e}")))?;
    }
    Ok(POOL.get_or_init(|| pool))
}

/// The operation and the request and answer descriptors of `path`
/// (`/pcx.v1.PostCortex/CreateSession`).
fn method(
    path: &str,
) -> Result<(&'static Operation, MessageDescriptor, MessageDescriptor), ClassifyError> {
    let unknown = || ClassifyError::UnknownRpc(path.to_string());
    let (service, rpc) = path
        .trim_start_matches('/')
        .split_once('/')
        .ok_or_else(unknown)?;
    let operation = OPERATIONS
        .iter()
        .find(|op| match service {
            PCX => op.grpc.contains(&rpc),
            BLOBS => op.blobs.contains(&rpc),
            VIEWS => op.views.contains(&rpc),
            _ => false,
        })
        .ok_or_else(unknown)?;
    let method = pool()?
        .get_service_by_name(service)
        .and_then(|s| s.methods().find(|m| m.name() == rpc))
        .ok_or_else(unknown)?;
    Ok((operation, method.input(), method.output()))
}

fn decode(descriptor: MessageDescriptor, message: &[u8]) -> Result<DynamicMessage, ClassifyError> {
    let mut decoded = DynamicMessage::decode(descriptor, message)
        .map_err(|e| ClassifyError::Malformed(e.to_string()))?;
    without_any(&mut decoded);
    Ok(decoded)
}

/// `message` without its `google.protobuf.Any` fields, at any depth: a
/// document's data is a message of its module's package, which this pool
/// does not hold, and no operation reads it.
fn without_any(message: &mut DynamicMessage) {
    let fields: Vec<_> = message.descriptor().fields().collect();
    for field in fields {
        if let Kind::Message(of) = field.kind() {
            if of.full_name() == "google.protobuf.Any" {
                message.clear_field(&field);
            } else if !field.is_list()
                && !field.is_map()
                && message.has_field(&field)
                && let Field::Message(inner) = message.get_field_mut(&field)
            {
                without_any(inner);
            }
        }
    }
}

/// `message` in its JSON form, with the proto field names.
fn to_json(message: &DynamicMessage) -> Result<Value, ClassifyError> {
    let options = SerializeOptions::new()
        .use_proto_field_name(true)
        .stringify_64_bit_integers(false);
    message
        .serialize_with_options(serde_json::value::Serializer, &options)
        .map_err(|e| ClassifyError::Malformed(e.to_string()))
}

/// The gRPC call `path` (`/pcx.v1.PostCortex/CreateSession`) with request
/// `message` (for a client stream, its first message).
///
/// # Errors
/// An unknown RPC, a message that does not decode, or a field that names
/// no id.
pub fn grpc(path: &str, message: &[u8]) -> Result<Classified, ClassifyError> {
    let (operation, input, _) = method(path)?;
    of_json(operation, &to_json(&decode(input, message)?)?)
}

/// Whether the RPC `path` is an operation that changes memory (a write or
/// a delete of the table); an RPC that is none of the table's does not.
#[must_use]
pub fn changes(path: &str) -> bool {
    method(path).is_ok_and(|(operation, _, _)| {
        matches!(
            operation.effect,
            Effect::Write { .. } | Effect::Delete { .. }
        )
    })
}

/// The count in `field` of the answer `message` of `path`.
///
/// # Errors
/// An unknown RPC, an answer that does not decode, or a field that is not
/// a count.
pub fn answer_count(path: &str, message: &[u8], field: &str) -> Result<i64, ClassifyError> {
    let (_, _, output) = method(path)?;
    json::answer_count(&to_json(&decode(output, message)?)?, field)
}

/// The flag in `field` of the answer `message` of `path` (false when the
/// message leaves it at its default).
///
/// # Errors
/// An unknown RPC, an answer that does not decode, or a field it does not
/// have or that is not a boolean.
pub fn answer_flag(path: &str, message: &[u8], field: &str) -> Result<bool, ClassifyError> {
    let (_, _, output) = method(path)?;
    let answer = decode(output.clone(), message)?;
    let not_a_flag = || ClassifyError::Malformed(format!("no boolean field {field}"));
    let field = output.get_field_by_name(field).ok_or_else(not_a_flag)?;
    answer.get_field(&field).as_bool().ok_or_else(not_a_flag)
}

/// The request `message` of `path` without its reranker: its rerank field
/// set to the fallback. An RPC that never reranks gets its message back.
///
/// # Errors
/// An unknown RPC or a message that does not decode.
pub fn without_rerank(path: &str, message: &[u8]) -> Result<Vec<u8>, ClassifyError> {
    let (operation, input, _) = method(path)?;
    let Effect::Search {
        rerank: Some(rerank),
    } = operation.effect
    else {
        return Ok(message.to_vec());
    };
    let mut request = decode(input.clone(), message)?;
    let field = input
        .get_field_by_name(rerank.field)
        .ok_or_else(|| ClassifyError::Malformed(format!("no field {}", rerank.field)))?;
    let value = match field.kind().as_enum() {
        Some(kind) => Field::EnumNumber(
            kind.get_value_by_name(rerank.fallback)
                .ok_or_else(|| ClassifyError::Malformed(format!("no value {}", rerank.fallback)))?
                .number(),
        ),
        None => Field::String(rerank.fallback.to_string()),
    };
    request.set_field(&field, value);
    Ok(prost::Message::encode_to_vec(&request))
}
