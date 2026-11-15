// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The Anthropic model provider (docs/modules/agent.md, the providers):
//! Claude models over Anthropic's Messages API, streamed, with adaptive
//! thinking where the model has it, tool use and the prompt cache (an API
//! key only: owner 2026-10-31); its listing (`GET /v1/models`) gives each
//! model's window and what it can do, its table the prices. A library of
//! the `models` module, which starts it with its API key.

#![forbid(unsafe_code)]

mod answer;
mod catalog;
mod http;
mod provider;
mod request;

pub use catalog::API as ENDPOINT;
pub use provider::Anthropic;

/// The provider's name, which its models carry.
pub const ID: &str = "anthropic";
