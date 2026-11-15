// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! How the sandbox runs the modules' processes (`[sandbox]`, docs/host.md,
//! sandbox and tools).

use serde::{Deserialize, Serialize};

/// The `[sandbox]` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxConfig {
    /// The unprivileged user a FreeBSD jail runs a process as (without
    /// one every process is refused there; no other platform takes one).
    pub user: String,
}

/// The `[dynamic]` table: the daemon runs dynamic modules, through a launcher
/// it serves itself (docs/modules/dynamic.md).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DynamicConfig {
    /// Free-threaded CPython with the binding `pcx-module` installed.
    pub python: std::path::PathBuf,
    /// Deno.
    pub deno: std::path::PathBuf,
    /// The import map giving a TS module `@post-cortex/module` and `@bufbuild/protobuf`.
    pub imports: std::path::PathBuf,
    /// What those runtimes read: their directories.
    pub read: Vec<std::path::PathBuf>,
    /// The protobuf compiler.
    pub protoc: std::path::PathBuf,
    /// Its plugin for TypeScript.
    pub protoc_gen_es: std::path::PathBuf,
    /// The workspace the modules are kept in.
    pub workspace_id: String,
    /// The board a person approves them on.
    pub board: String,
}
