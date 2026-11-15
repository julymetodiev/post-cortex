// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The workflow module's adapters: its definitions, runs and state over
//! memory's documents, and the rest over the host.

mod documents;
mod dry;
mod host;
mod own;
mod runs;

pub use documents::DocumentWorkflows;
pub use dry::DryStores;
pub use host::{Host, SystemClock};
pub use own::OwnDocuments;
pub use runs::DocumentRuns;
