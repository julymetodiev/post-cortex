// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What the chief needs of the world besides its documents and sessions
//! (`pcx-documents`) and the agent's turns (`pcx_module_api::agent::Turns`):
//! the day, for its budget.

/// The clock.
pub trait Clock: Send + Sync {
    /// Today's UTC date, `YYYY-MM-DD`.
    fn today(&self) -> String;
}
