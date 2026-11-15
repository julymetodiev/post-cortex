// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The `tools-fs` module (ARCHITECTURE section 7, sandbox and tools): read,
//! write, edit, list_dir, grep and glob, every one over the module's
//! sandbox, so beneath its roots. The tool handlers map JSON onto the file
//! service ([`files`]), which holds the logic over the `Sandbox` port.
//! Ported from Axon's file tools; grep and glob walk the sandbox instead of
//! running `rg` and `fd`.

#![forbid(unsafe_code)]

mod files;
mod handlers;
mod module;
mod specs;

pub use module::FsModule;

#[cfg(test)]
mod tests;
