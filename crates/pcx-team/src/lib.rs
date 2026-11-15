// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The `team` module (ARCHITECTURE section 8): agents with roles and
//! authority, and teams of them, kept as documents in memory. Its domain
//! types are the documents' shape, which the board reads too to check who
//! may decide or close.

#![forbid(unsafe_code)]

mod adapters;
mod autonomy;
mod doing;
mod domain;
mod ending;
mod going;
mod handlers;
mod module;
mod place;
mod ports;
mod progress;
mod runner;
mod schemas;
mod service;
mod specs;
mod steps;
mod turns;
mod view;
mod wake;
mod work;

pub use autonomy::{
    Autonomy, Class, CodeLimit, DataLimit, Exception, Mode, MoneyLimit, Refusal, Stakes, Trust,
    verdict,
};
pub use domain::{Act, Agent, Authority, Kind as TeamKind, Refused, Team, may};
pub use module::TeamModule;
