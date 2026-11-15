// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A pcx module's documents in memory (ARCHITECTURE section 8, documents
//! of a module, shared): the [`Documents`] port (get, put from an expected
//! version, a page or all of a kind by an example's fields), [`Typed`] reads and writes of a
//! kind's message over it, and its adapters: [`HostDocuments`] over
//! memory's `pcx.v1` documents through the module's host channel (W23),
//! [`SharedDocuments`] for a store that is the organisation's, whoever
//! calls, and [`InMemoryDocuments`], versioned as memory versions them; and a
//! module's new sessions in its workspace ([`Sessions`], over memory in
//! [`HostSessions`]); the days of a log of documents ([`days`]); and every
//! workspace of the host, for a module's own work across them
//! ([`every_workspace`]).

#![forbid(unsafe_code)]

pub mod days;
mod host;
mod in_memory;
mod ports;
mod sessions;
mod shared;
mod typed;
mod workspaces;

pub use host::HostDocuments;
pub use in_memory::InMemoryDocuments;
pub use ports::{Documents, Page, Sessions};
pub use sessions::HostSessions;
pub use shared::SharedDocuments;
pub use typed::{Item, Paged, Typed};
pub use workspaces::every_workspace;
