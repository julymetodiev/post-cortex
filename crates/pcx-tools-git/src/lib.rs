// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The `tools-git` module (ARCHITECTURE section 7, sandbox and tools):
//! status, diff, log, branches, commits and the commits of a branch, as the
//! `git` program run through the module's sandbox. Axon's parsing and
//! Conventional Commits are ported; `git2` is not used.

#![forbid(unsafe_code)]

mod git;
mod handlers;
mod module;
mod parse;
mod push;
mod specs;

pub use module::GitModule;

// git runs under the OS sandbox: Seatbelt in tests; Landlock needs the
// `pcx` binary's helper (the daemon's tests).
#[cfg(all(test, target_os = "macos"))]
mod tests;
