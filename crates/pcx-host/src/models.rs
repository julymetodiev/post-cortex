// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The models of the running providers as one [`ModelProvider`]
//! (ARCHITECTURE section 7, the agent): each model named by its provider
//! (as its module names it, else the module's id); a request goes to the
//! first provider that lists its model (once the listings begun at start
//! are in, when none lists it yet), under that provider's policy until its
//! answer starts (its breaker, the request's deadline, a panic caught);
//! the answer streams from there.

use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use pcx_module_api::Scope;
use pcx_module_api::model::{
    ModelFailure, ModelFailureKind, ModelInfo, ModelProvider, ModelRequest, ModelRun,
};

use tokio::time::timeout_at;

use crate::policy::Guarded;

/// A running provider: its module's guard, and its models.
type Running = (Arc<Guarded>, Arc<dyn ModelProvider>);

/// The providers of one running set, in load order.
pub struct Models {
    providers: Vec<Running>,
}

impl Models {
    /// The providers among `modules`.
    pub fn of(modules: &[Arc<Guarded>]) -> Self {
        Self {
            providers: modules
                .iter()
                .filter_map(|g| Some((Arc::clone(g), g.module().model_provider()?)))
                .collect(),
        }
    }

    /// The provider that serves `model`: as listed now, else once the
    /// listings begun at start are in (a provider started a moment ago
    /// lists its table's models until its API's listing is in), until
    /// `deadline`.
    async fn serving(&self, model: &str, deadline: Instant) -> Option<&Running> {
        let lists = |models: &[ModelInfo]| models.iter().any(|m| m.id == model);
        if let Some(found) = self.providers.iter().find(|(_, p)| lists(&p.models())) {
            return Some(found);
        }
        for running in &self.providers {
            // A refused listing, or one past the deadline, serves nothing.
            if let Ok(Ok(listed)) = timeout_at(deadline.into(), running.1.listed()).await
                && lists(&listed)
            {
                return Some(running);
            }
        }
        None
    }
}

#[async_trait]
impl ModelProvider for Models {
    fn models(&self) -> Vec<ModelInfo> {
        self.providers
            .iter()
            .flat_map(|(g, p)| {
                p.models().into_iter().map(|model| ModelInfo {
                    provider: if model.provider.is_empty() {
                        g.id().to_string()
                    } else {
                        model.provider.clone()
                    },
                    ..model
                })
            })
            .collect()
    }

    async fn generate(
        &self,
        scope: &Scope,
        request: ModelRequest,
    ) -> Result<ModelRun, ModelFailure> {
        let (guarded, provider) = self
            .serving(&request.model, scope.deadline)
            .await
            .ok_or_else(|| {
                ModelFailure::new(
                    ModelFailureKind::Invalid,
                    format!("no running provider serves the model {}", request.model),
                )
            })?;
        let (provider, asked) = (Arc::clone(provider), scope.clone());
        guarded
            .run(&scope.cancel, scope.deadline, "generate", |_| {
                Box::pin(async move { Ok(provider.generate(&asked, request).await) })
            })
            .await
            // The provider's policy refused or caught it: not reached.
            .map_err(|e| ModelFailure::new(ModelFailureKind::Unavailable, e.message))?
    }
}
