// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The OpenRouter model provider (docs/modules/agent.md, the providers):
//! every model OpenRouter routes, over its OpenAI-compatible chat
//! completions API (`pcx-openai-compat`); its listing gives each model's
//! modalities, window and price. A library of the `models` module, which
//! starts it with its API key.

#![forbid(unsafe_code)]

mod catalog;

pub use catalog::ENDPOINT;
use pcx_module_api::model::Table;
use pcx_openai_compat::{Provider, Wire};
use serde_json::Map;

/// The provider's name, which its models carry.
pub const ID: &str = "openrouter";

/// OpenRouter's provider, not started.
#[must_use]
pub fn provider() -> Provider {
    let wire = Wire {
        max_tokens: "max_tokens",
        extra: Map::new(),
    };
    // Its listing describes every model: no table.
    Provider::new(
        ID,
        wire,
        catalog::codes(),
        Table::default(),
        catalog::served,
    )
}
