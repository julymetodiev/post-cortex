// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A model provider called with an API key: the `models` module starts
//! each one whose key its settings set (docs/modules/agent.md, the
//! providers).

use async_trait::async_trait;

use crate::ModuleError;
use crate::model::ModelProvider;

/// A provider its module starts with its endpoint and key.
#[async_trait]
pub trait KeyedProvider: ModelProvider {
    /// Serve through `url` with `key` from now on, its listing begun.
    ///
    /// # Errors
    /// `InvalidArgument` (on `api_key`) when the key is refused;
    /// `Unavailable` when its client cannot be built.
    async fn start(&self, url: &str, key: &str) -> Result<(), ModuleError>;

    /// Stop serving.
    fn stop(&self);
}
