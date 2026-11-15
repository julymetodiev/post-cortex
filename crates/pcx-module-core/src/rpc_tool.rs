// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A module's RPCs as the tools agents and models call (ARCHITECTURE
//! section 7, everything gRPC): the one place JSON meets the typed world.
//! Each RPC a manifest names as a tool (`Manifest.rpcs`) is listed with its
//! description (the RPC's comment in the module's descriptors) and the
//! JSON Schema of its request message; a call's JSON is read into that
//! message, and its answer written back, with the canonical proto JSON
//! mapping (proto field names), and the call itself goes through a channel
//! to the host like any other gRPC call, routed by path to the tool's
//! module and checked. The
//! host's tool bridge and a module process's handle both call through it.

use prost::Message;
use prost_reflect::{
    DescriptorPool, DynamicMessage, MessageDescriptor, MethodDescriptor, ReflectMessage,
    SerializeOptions,
};
use serde_json::Value;
use tonic::codegen::http::uri::PathAndQuery;
use tonic::metadata::AsciiMetadataValue;

use crate::grpc::GrpcChannel;
use crate::tool::effect_from_wire;
use crate::{Manifest, ModuleError, ModuleErrorKind, Scope, ToolOutput, ToolSpec, Visibility};

mod pools;
mod schema;

pub use pools::pools;

/// One RPC of a module, as a tool.
#[derive(Debug, Clone)]
pub struct RpcTool {
    /// The tool agents see.
    pub spec: ToolSpec,
    /// Its gRPC path, `/pcx.board.v1.Boards/AddTask`.
    path: PathAndQuery,
    /// Its module's id, the target of its calls: another module may serve
    /// the same service (docs/host.md, a service several modules serve).
    module: AsciiMetadataValue,
    /// Its request message.
    request: MessageDescriptor,
    /// Its response message.
    response: MessageDescriptor,
}

/// The method of `pool` whose gRPC path is `path`.
fn method(pool: &DescriptorPool, path: &str) -> Option<MethodDescriptor> {
    pool.services()
        .flat_map(|service| service.methods().collect::<Vec<_>>())
        .find(|m| format!("/{}/{}", m.parent_service().full_name(), m.name()) == path)
}

/// The tools of the RPCs `manifest` names as tools, built from its own
/// descriptors, as [`tools_in`].
///
/// # Errors
/// As [`tools_in`]; its descriptors do not read.
pub fn tools(manifest: &Manifest) -> Result<Vec<RpcTool>, String> {
    tools_in(manifest, &crate::descriptors(&manifest.descriptors)?)
}

/// The tools of the RPCs `manifest` names as tools, in its order, their
/// messages those of `pool` (its descriptors, alone or with other
/// modules': [`pools`]); every RPC it names, a tool or not, states its
/// effect.
///
/// # Errors
/// Why the manifest is not one: an RPC without an effect, an RPC is not a
/// method of `pool`, or a visibility is not one.
pub fn tools_in(manifest: &Manifest, pool: &DescriptorPool) -> Result<Vec<RpcTool>, String> {
    for rpc in &manifest.rpcs {
        effect_from_wire(rpc.effect, &format!("its rpc {}", rpc.method)).map_err(|e| e.message)?;
    }
    let named: Vec<_> = manifest
        .rpcs
        .iter()
        .filter(|r| !r.tool.is_empty())
        .collect();
    if named.is_empty() {
        return Ok(Vec::new());
    }
    let module = AsciiMetadataValue::try_from(manifest.id.as_str())
        .map_err(|_| format!("its id {} is not ASCII text", manifest.id))?;
    named
        .into_iter()
        .map(|rpc| {
            let found = method(pool, &rpc.method).ok_or_else(|| {
                format!("its rpc {} is not a method of its descriptors", rpc.method)
            })?;
            let visibility =
                Visibility::from_wire(&rpc.visibility, &rpc.tool).map_err(|e| e.message)?;
            let effect = effect_from_wire(rpc.effect, &rpc.tool).map_err(|e| e.message)?;
            let path = PathAndQuery::try_from(rpc.method.as_str())
                .map_err(|e| format!("its rpc {}: {e}", rpc.method))?;
            Ok(RpcTool {
                spec: ToolSpec {
                    name: rpc.tool.clone(),
                    description: schema::comments(&found),
                    input_schema: schema::object(&found.input()),
                    visibility,
                    effect,
                },
                path,
                module: module.clone(),
                request: found.input(),
                response: found.output(),
            })
        })
        .collect()
}

fn invalid(reason: String) -> ModuleError {
    ModuleError::new(ModuleErrorKind::InvalidArgument, reason)
}

impl RpcTool {
    /// Its gRPC path.
    #[must_use]
    pub fn path(&self) -> &str {
        self.path.as_str()
    }

    /// Its request message, which a workflow's template is checked
    /// against (docs/modules/workflow.md, the check).
    #[must_use]
    pub fn request(&self) -> &MessageDescriptor {
        &self.request
    }

    /// Its response message: a workflow block's output.
    #[must_use]
    pub fn response(&self) -> &MessageDescriptor {
        &self.response
    }

    /// The JSON Schema of its response message, as its input's is written
    /// (the catalog of a workflow's blocks, docs/modules/workflow.md).
    #[must_use]
    pub fn response_schema(&self) -> serde_json::Map<String, serde_json::Value> {
        schema::object(&self.response)
    }

    /// Call it through `channel` in `scope` with `request`, a message of
    /// its request type, and `idempotency_key` when it must act once: its
    /// response (a workflow's `tool` block, docs/modules/workflow.md).
    ///
    /// # Errors
    /// `InvalidArgument` for a request of another type; the call's status;
    /// `Internal` for an answer that is not the response.
    pub async fn call_message(
        &self,
        channel: GrpcChannel,
        scope: &Scope,
        request: &DynamicMessage,
        idempotency_key: Option<&str>,
    ) -> Result<DynamicMessage, ModuleError> {
        let name = &self.spec.name;
        if request.descriptor().full_name() != self.request.full_name() {
            return Err(invalid(format!(
                "{name}: the request is a {}, not a {}",
                request.descriptor().full_name(),
                self.request.full_name()
            )));
        }
        let mut call = tonic::Request::new(request.encode_to_vec());
        scope.to_metadata(call.metadata_mut())?;
        call.metadata_mut()
            .insert(crate::scope::metadata::TARGET, self.module.clone());
        if let Some(key) = idempotency_key {
            let value = AsciiMetadataValue::try_from(key)
                .map_err(|_| invalid(format!("{name}: the idempotency key is not ASCII text")))?;
            call.metadata_mut()
                .insert(crate::scope::metadata::IDEMPOTENCY_KEY, value);
        }
        let answer = crate::grpc::unary(channel, self.path.clone(), call).await?;
        DynamicMessage::decode(self.response.clone(), answer.as_slice()).map_err(|e| {
            ModuleError::new(
                ModuleErrorKind::Internal,
                format!(
                    "{name} answered what is not a {}: {e}",
                    self.response.full_name()
                ),
            )
        })
    }

    /// Call it through `channel` in `scope` with the JSON `input`: its
    /// answer as JSON, its data (an RPC's answer has no text for a person).
    ///
    /// # Errors
    /// `InvalidArgument` for input that does not read as the request; the
    /// call's status; `Internal` for an answer that is not the response.
    pub async fn call(
        &self,
        channel: GrpcChannel,
        scope: &Scope,
        input: Value,
    ) -> Result<ToolOutput, ModuleError> {
        let name = &self.spec.name;
        let input = match input {
            Value::Null => Value::Object(serde_json::Map::new()),
            input => input,
        };
        let message = DynamicMessage::deserialize(self.request.clone(), &input).map_err(|e| {
            // The decoder names a key it does not know, not where it is.
            let reason =
                schema::unknown_field(&self.request, &input).unwrap_or_else(|| e.to_string());
            invalid(format!(
                "{name}: the input is not a {}: {reason}",
                self.request.full_name()
            ))
        })?;
        let response = self.call_message(channel, scope, &message, None).await?;
        let options = SerializeOptions::new()
            .use_proto_field_name(true)
            .skip_default_fields(false);
        let data = response
            .serialize_with_options(serde_json::value::Serializer, &options)
            .map_err(|e| {
                ModuleError::new(
                    ModuleErrorKind::Internal,
                    format!("{name}: its answer does not map to JSON: {e}"),
                )
            })?;
        Ok(ToolOutput::answer(String::new(), data))
    }
}
