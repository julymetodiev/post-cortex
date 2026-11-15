// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The core of a pcx module process (docs/modules/dynamic.md, the core):
//! what a module speaks with its host without the in-process host types,
//! shared by Rust process modules (through `pcx-module-api`) and the
//! bindings of other languages. The module's errors ([`ModuleError`]),
//! [`Scope`], the gRPC channel ([`grpc`]), its log ring, the tools it
//! declares ([`Effect`], [`Visibility`]), configuration types and their
//! checks ([`ConfigType`]), the tools of a manifest ([`rpc_tool`]), the
//! bus ([`bus`]), its callback socket ([`Callback`]), the
//! `pcx.module.v1.Module` server ([`serve`] of a [`Process`]) and the
//! inverted control of a module in another language ([`handoff`]).

#![forbid(unsafe_code)]

pub mod bus;
mod callback;
pub mod channel;
mod config;
mod error;
mod grant;
pub mod grpc;
pub mod handoff;
pub mod json;
mod log;
mod manifest;
pub mod name;
pub mod rpc_tool;
mod scope;
mod serve;
mod tool;

pub use callback::Callback;
pub use config::{
    ConfigError, ConfigType, DeclaredKind, FormSchema, KindError, SchemaError, data_matches,
    descriptors,
};
pub use error::{ModuleError, ModuleErrorKind, OFFLINE_METADATA};
pub use grant::TurnGrant;
pub use handoff::{DESCRIPTORS_ENV, DESCRIPTORS_FILE};
pub use log::{LOG_LINES, LogLine, LogRing, LogSource};
pub use manifest::{API_VERSION, EVERY_MODULE, compatible, uses};
pub use post_cortex_proto::module::{
    ApiVersion, Capability, DocumentKind, Event, EventEnvelope, EventKind, FailurePolicy,
    FilesystemGrant, HookSpecificOutput, Manifest, MemoryFilter, ModuleKind, Outcome, PersonRole,
    Prompt, PromptArgument, PromptContent, PromptMessage, PromptRole, Resource, Rpc, Subscription,
};
pub use scope::{AGENTS_DOMAIN, AgentAuthor, MAX_CALL_DEPTH, Scope, metadata, narrower};
pub use serve::{DEFAULT_DEADLINE, Process, SOCKET_ENV, ServeError, Start, logs, serve};
pub use tool::{Effect, ToolOutput, ToolSpec, Visibility, agent_may_call, effect_from_wire};
