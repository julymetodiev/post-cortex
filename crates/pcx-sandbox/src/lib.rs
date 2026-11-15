// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The pcx sandbox (ARCHITECTURE section 7, sandbox and tools): the module
//! of the `sandbox` slot ([`SandboxModule`]). A module's files resolve
//! beneath its roots through `cap-std`; its processes run under the OS
//! sandbox ([`Launcher`]: Seatbelt on macOS; through the `pcx sandbox-run`
//! helper (`run_helper`) Landlock and seccomp on Linux, a jail on
//! FreeBSD; refused elsewhere).

#![forbid(unsafe_code)]

mod bound;
mod deny;
mod exec;
mod files;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
mod helper;
#[cfg(target_os = "freebsd")]
mod jail;
#[cfg(target_os = "linux")]
mod landlock;
mod launcher;
mod module;
#[cfg(target_os = "macos")]
mod seatbelt;

pub use deny::{Deny, DenyError};
#[cfg(target_os = "freebsd")]
pub use jail::{RunError, run as run_helper};
#[cfg(target_os = "linux")]
pub use landlock::{RunError, run as run_helper};
pub use launcher::{Launcher, Limits};
pub use module::SandboxModule;

#[cfg(all(test, target_os = "macos"))]
mod tests;
