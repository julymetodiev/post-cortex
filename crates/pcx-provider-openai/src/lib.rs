// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The OpenAI model provider (docs/modules/agent.md, the providers):
//! OpenAI's models over its chat completions API (`pcx-openai-compat`).
//! Its listing gives only ids, so its table describes the models it knows.
//! A library of the `models` module, which starts it with its API key.

#![forbid(unsafe_code)]

mod catalog;

pub use catalog::ENDPOINT;
use pcx_openai_compat::{Provider, Wire, served};
use serde_json::Map;

/// The provider's name, which its models carry.
pub const ID: &str = "openai";

/// OpenAI's provider, not started.
#[must_use]
pub fn provider() -> Provider {
    // Its reasoning models refuse `max_tokens`.
    let wire = Wire {
        max_tokens: "max_completion_tokens",
        extra: Map::new(),
    };
    Provider::new(ID, wire, catalog::codes(), catalog::table(), served)
}
