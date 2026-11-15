// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A model provider on an OpenAI-compatible endpoint: what Z.AI, OpenAI
//! and OpenRouter share. Each gives its table, how it reads its listing,
//! and what its bodies add ([`Wire`]); the provider begins listing its
//! models at start, in the background (docs/modules/agent.md, choosing the
//! models), and sends each request to the endpoint.

use async_trait::async_trait;
use parking_lot::RwLock;
use pcx_module_api::model::{
    KeyedProvider, Listed, ModelFailure, ModelFailureKind, ModelInfo, ModelProvider, ModelRequest,
    ModelRun, Table,
};
use pcx_module_api::{ModuleError, ModuleErrorKind, Scope};
use serde_json::{Map, Value};

use crate::{Codes, Endpoint, body};

/// What a provider's bodies carry beyond the common ones.
#[derive(Debug, Clone)]
pub struct Wire {
    /// The field the most tokens of the answer go in (`max_tokens`;
    /// OpenAI's reasoning models take `max_completion_tokens`).
    pub max_tokens: &'static str,
    /// Its own fields, merged at the body's root (Z.AI's
    /// `enable_thinking`).
    pub extra: Map<String, Value>,
}

/// How a provider reads its listing's JSON into the models it serves.
pub type Listing = fn(&Value) -> Vec<ModelInfo>;

/// A provider on an OpenAI-compatible endpoint.
pub struct Provider {
    id: &'static str,
    wire: Wire,
    codes: Codes,
    table: Table,
    listing: Listing,
    endpoint: RwLock<Option<Endpoint>>,
    /// Its listing: the table's until its API's is in.
    models: Listed,
}

impl Provider {
    /// The provider `id` (its module's), not started: its bodies as `wire`
    /// has them, its failures by `codes`.
    #[must_use]
    pub fn new(id: &'static str, wire: Wire, codes: Codes, table: Table, listing: Listing) -> Self {
        let models = table.listing(None);
        Self {
            id,
            wire,
            codes,
            table,
            listing,
            endpoint: RwLock::new(None),
            models: Listed::new(models),
        }
    }
}

#[async_trait]
impl ModelProvider for Provider {
    fn models(&self) -> Vec<ModelInfo> {
        self.models.models()
    }

    async fn listed(&self) -> Result<Vec<ModelInfo>, ModuleError> {
        self.models.listed().await
    }

    async fn generate(
        &self,
        scope: &Scope,
        request: ModelRequest,
    ) -> Result<ModelRun, ModelFailure> {
        let endpoint = self.endpoint.read().clone().ok_or_else(|| {
            ModelFailure::new(
                ModelFailureKind::Unavailable,
                format!("{} has not started", self.id),
            )
        })?;
        endpoint.generate(scope, &body(&request, &self.wire)?).await
    }
}

#[async_trait]
impl KeyedProvider for Provider {
    async fn start(&self, url: &str, key: &str) -> Result<(), ModuleError> {
        let endpoint = Endpoint::new(url, key, self.codes.clone())
            .map_err(|f| ModuleError::new(ModuleErrorKind::Unavailable, f.message))?;
        let (listing, lists) = (self.listing, endpoint.clone());
        let served = async move { lists.models().await.map(|l| listing(&l)) };
        self.models.begin(self.id, self.table.clone(), served);
        *self.endpoint.write() = Some(endpoint);
        Ok(())
    }

    fn stop(&self) {
        *self.endpoint.write() = None;
        self.models.stop();
    }
}
