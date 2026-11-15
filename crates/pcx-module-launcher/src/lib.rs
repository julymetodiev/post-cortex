// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The adapters of `pcx-host`'s port `ModuleLauncher` for a module that is
//! not first-party (docs/modules/dynamic.md, where it runs): [`Confined`]
//! under the OS sandbox with its limits; [`Served`], the launcher of the pot
//! `pcx-modules` (`pcx module-launcher`), and [`Remote`], its client.

#![forbid(unsafe_code)]

mod bundle;
mod confined;
mod lock;
mod prepare;
mod protoc;
mod remote;
mod runtimes;
mod served;
mod watch;

pub use confined::{Confined, Confinement, Ran};
pub use prepare::{PrepareConfig, PrepareError, Prepared, Preparer};
pub use remote::{Remote, Requested, prepare};
pub use runtimes::{Run, Runtimes};
pub use served::{Served, ServedConfig, serve};
pub use watch::Watcher;
