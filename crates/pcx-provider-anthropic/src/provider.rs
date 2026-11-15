// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The provider: its listing begun at start, in the background
//! (docs/modules/agent.md, choosing the models), each request as a
//! Messages body, adaptive thinking asked of the models that have it.

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::RwLock;
use pcx_module_api::model::{
    KeyedProvider, Listed, ModelFailure, ModelFailureKind, ModelInfo, ModelProvider, ModelRequest,
    ModelRun,
};
use pcx_module_api::{ModuleError, ModuleErrorKind, Scope};

use crate::http::Endpoint;
use crate::{ID, catalog, request};

/// Anthropic's models.
pub struct Anthropic {
    endpoint: RwLock<Option<Endpoint>>,
    /// The models that think adaptively, by id: its table's, and the
    /// listed ones whose capabilities say so once its listing is in.
    adaptive: Arc<RwLock<HashSet<String>>>,
    /// Its listing: the table's until Anthropic's is in.
    models: Listed,
}

impl Default for Anthropic {
    fn default() -> Self {
        Self {
            endpoint: RwLock::new(None),
            adaptive: Arc::default(),
            models: Listed::new(catalog::table().listing(None)),
        }
    }
}

#[async_trait]
impl ModelProvider for Anthropic {
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
                format!("{ID} has not started"),
            )
        })?;
        let thinks = self.adaptive.read().contains(&request.model);
        endpoint
            .generate(scope, &request::body(&request, thinks)?)
            .await
    }
}

#[async_trait]
impl KeyedProvider for Anthropic {
    async fn start(&self, url: &str, key: &str) -> Result<(), ModuleError> {
        let endpoint = Endpoint::new(url, key)
            .map_err(|f| ModuleError::new(ModuleErrorKind::Unavailable, f.message))?;
        let table = catalog::table();
        // The models of its table think adaptively; a listed one when its
        // capabilities say so.
        *self.adaptive.write() = table.known.iter().map(|m| m.id.clone()).collect();
        let (lists, adaptive) = (endpoint.clone(), Arc::clone(&self.adaptive));
        let served = async move {
            lists.models().await.map(|entries| {
                adaptive.write().extend(
                    entries
                        .iter()
                        .filter(|e| catalog::adaptive(e))
                        .filter_map(|e| e.get("id").and_then(serde_json::Value::as_str))
                        .map(str::to_string),
                );
                entries.iter().filter_map(catalog::model).collect()
            })
        };
        self.models.begin(ID, table, served);
        *self.endpoint.write() = Some(endpoint);
        Ok(())
    }

    fn stop(&self) {
        *self.endpoint.write() = None;
        self.models.stop();
    }
}

#[cfg(test)]
mod tests;
