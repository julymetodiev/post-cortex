// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The pcx module API (ARCHITECTURE section 7): what a module implements
//! ([`Module`]) and what it speaks with the host ([`Scope`], the
//! `pcx.module.v1` [`Manifest`], [`Event`] and [`Outcome`]; [`ToolSpec`],
//! [`ToolOutput`] and [`ModuleError`]; a configuration's [`ConfigType`];
//! the [`view`]s it declares).
//! In-process modules implement [`Module`]; out-of-process ones serve the
//! same contract over `pcx.module.v1` ([`serve`]). What a module process
//! speaks without the in-process host is `pcx-module-core`'s
//! (docs/modules/dynamic.md), re-exported here.

#![forbid(unsafe_code)]

pub mod agent;
pub mod bus;
pub mod context;
mod daily;
mod hook;
mod memory_write;
pub mod model;
mod module;
mod module_available;
mod remote;
pub mod sandbox;
mod serve;
pub mod text;
pub mod view;

pub use daily::daily;
pub use hook::event_kind;
pub use memory_write::{MEMORY_WRITE, MemoryWrite};
pub use module::{HostContext, Module, ModuleHost, ThroughHost, UsedTool};
pub use module_available::{MODULE_AVAILABLE, ModuleAvailable};
pub use pcx_module_core::{
    AGENTS_DOMAIN, API_VERSION, AgentAuthor, ApiVersion, Capability, ConfigError, ConfigType,
    DEFAULT_DEADLINE, DESCRIPTORS_ENV, DESCRIPTORS_FILE, DeclaredKind, DocumentKind, EVERY_MODULE,
    Effect, Event, EventEnvelope, EventKind, FailurePolicy, FilesystemGrant, FormSchema,
    HookSpecificOutput, KindError, LOG_LINES, LogLine, LogRing, LogSource, MAX_CALL_DEPTH,
    Manifest, MemoryFilter, ModuleError, ModuleErrorKind, ModuleKind, OFFLINE_METADATA, Outcome,
    PersonRole, Prompt, PromptArgument, PromptContent, PromptMessage, PromptRole, Resource, Rpc,
    SOCKET_ENV, SchemaError, Scope, ServeError, Subscription, ToolOutput, ToolSpec, Visibility,
    agent_may_call, channel, compatible, data_matches, descriptors, effect_from_wire, grpc, json,
    metadata, name, narrower, rpc_tool, uses,
};
pub use serve::{main, serve};

#[cfg(test)]
mod tests;
