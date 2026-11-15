// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The OpenAI-compatible chat completions wire (ARCHITECTURE section 7,
//! the agent), shared by every provider that speaks it (Z.AI first): the
//! request body of a [`pcx_module_api::model::ModelRequest`] ([`body`]),
//! the answer its server-sent events build ([`Answer`]), the call over
//! HTTP with its failures classified by status and error code
//! ([`Endpoint`]), and the provider every such provider's crate
//! configures ([`Provider`]). Pure but for [`Endpoint`].

#![forbid(unsafe_code)]

mod answer;
mod http;
mod listing;
mod provider;
mod request;

pub use answer::Answer;
pub use http::{Codes, Endpoint};
pub use listing::served;
pub use provider::{Listing, Provider, Wire};
pub use request::body;
