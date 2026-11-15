// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The provider: Z.AI's chat completions with its reasoning on, its models
//! from its listing and its table.

use pcx_openai_compat::{Provider, Wire, served};
use serde_json::{Map, Value};

use crate::catalog;

/// The provider's name, which its models carry.
pub const ID: &str = "zai";

/// Z.AI's provider, not started.
#[must_use]
pub fn provider() -> Provider {
    // GLM's reasoning channel.
    let wire = Wire {
        max_tokens: "max_tokens",
        extra: Map::from_iter([("enable_thinking".to_string(), Value::Bool(true))]),
    };
    Provider::new(ID, wire, catalog::codes(), catalog::table(), served)
}

#[cfg(test)]
mod tests;
