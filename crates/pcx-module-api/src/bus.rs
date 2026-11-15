// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The organisation's bus as `pcx-module-core` declares it, and its
//! service on a module's callback socket ([`BusService`], through the
//! module's handle, [`crate::ModuleHost::bus`]).

mod serve;

pub use pcx_module_core::bus::*;
pub use serve::{BusService, BusSource};
