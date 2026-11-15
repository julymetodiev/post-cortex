// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The `tools-shell` module (ARCHITECTURE section 7, sandbox and tools):
//! `bash` runs one command and answers the tail of its output; the `tmux_*`
//! tools keep long-running commands in tmux sessions, whose tmux server
//! itself runs under the module's sandbox. Every process goes through the
//! `Sandbox` port, never `std::process`.

#![forbid(unsafe_code)]

mod handlers;
mod module;
mod shell;
mod specs;
mod tmux;

pub use module::ShellModule;

// Processes run under the OS sandbox: Seatbelt in tests; Landlock needs
// the `pcx` binary's helper (the daemon's tests).
#[cfg(all(test, target_os = "macos"))]
mod tests;
