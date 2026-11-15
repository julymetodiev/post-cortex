// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The Z.AI model provider (docs/modules/agent.md, the providers): GLM
//! models over Z.AI's coding endpoint, an OpenAI-compatible chat
//! completions API (`pcx-openai-compat`) with reasoning on. A library of
//! the `models` module, which starts it with its API key.

#![forbid(unsafe_code)]

mod catalog;
mod provider;

pub use catalog::ENDPOINT;
pub use provider::{ID, provider};
