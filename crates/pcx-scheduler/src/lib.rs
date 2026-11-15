// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The `scheduler` module (ARCHITECTURE section 8, operations, W28): jobs
//! at cron times or intervals that run a script under the sandbox or add a
//! board task (a member's work at a set time is a task assigned to it),
//! and their runs, kept as documents in memory. The domain says when a job
//! is due; the service keeps jobs and runs and does a run over its ports;
//! the runner looks for jobs due; the handlers only map.

#![forbid(unsafe_code)]

mod adapters;
mod domain;
mod handlers;
mod module;
mod ports;
mod runner;
mod service;
mod specs;

pub use module::SchedulerModule;
