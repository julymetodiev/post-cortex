// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The providers whose key the settings set, as one [`ModelProvider`]:
//! their models together, each with its provider's name; a request goes to
//! the first of them that lists its model.

use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::RwLock;
use pcx_module_api::model::{
    KeyedProvider, ModelFailure, ModelFailureKind, ModelInfo, ModelProvider, ModelRequest, ModelRun,
};
use pcx_module_api::{ModuleError, Scope};
use post_cortex_proto::models::{Keyed, ModelsConfig};

/// A provider and its section of the settings.
pub struct Section {
    /// Its name: its section's field, and the provider its models carry.
    pub name: &'static str,
    /// Its API's URL when its section names none.
    pub endpoint: &'static str,
    /// Its section of the settings.
    pub keyed: fn(&ModelsConfig) -> Option<&Keyed>,
    /// The provider.
    pub provider: Arc<dyn KeyedProvider>,
}

/// Every provider, and which of them are on.
pub struct Joined {
    sections: Vec<Section>,
    /// The sections started, by index, in the settings' order.
    on: RwLock<Vec<usize>>,
}

impl Joined {
    /// `sections`, none on.
    pub fn new(sections: Vec<Section>) -> Self {
        Self {
            sections,
            on: RwLock::new(Vec::new()),
        }
    }

    /// Start the providers whose key `config` sets, each through its
    /// section's endpoint or its own; stop the others. With no key set none
    /// is on and none is listed: the module runs, so its settings are where
    /// a person sets one (a module that does not start takes its host's
    /// others down with it).
    ///
    /// # Errors
    /// `InvalidArgument` when a provider refuses its key (on
    /// `<provider>.api_key`); `Unavailable` when a provider's client cannot
    /// be built.
    pub async fn start(&self, config: &ModelsConfig) -> Result<(), ModuleError> {
        self.stop();
        let mut on = Vec::new();
        for (index, section) in self.sections.iter().enumerate() {
            let Some(keyed) = (section.keyed)(config).filter(|k| !k.api_key.is_empty()) else {
                continue;
            };
            let url = if keyed.endpoint.is_empty() {
                section.endpoint
            } else {
                keyed.endpoint.as_str()
            };
            if let Err(error) = section.provider.start(url, &keyed.api_key).await {
                self.stop_all();
                return Err(of_section(section.name, error));
            }
            on.push(index);
        }
        *self.on.write() = on;
        Ok(())
    }

    /// Stop every provider.
    pub fn stop(&self) {
        self.on.write().clear();
        self.stop_all();
    }

    fn stop_all(&self) {
        for section in &self.sections {
            section.provider.stop();
        }
    }

    /// The sections on, in the settings' order.
    fn on(&self) -> Vec<&Section> {
        self.on
            .read()
            .iter()
            .filter_map(|i| self.sections.get(*i))
            .collect()
    }

    /// The provider on that serves `model`: as listed now, else once the
    /// listings begun at start are in (a provider started a moment ago).
    async fn serving(&self, model: &str) -> Option<Arc<dyn KeyedProvider>> {
        let on = self.on();
        if let Some(section) = on
            .iter()
            .find(|s| s.provider.models().iter().any(|m| m.id == model))
        {
            return Some(Arc::clone(&section.provider));
        }
        for section in on {
            // A refused listing serves nothing.
            if let Ok(listed) = section.provider.listed().await
                && listed.iter().any(|m| m.id == model)
            {
                return Some(Arc::clone(&section.provider));
            }
        }
        None
    }
}

/// `models` as `section`'s provider serves them.
fn named(section: &Section, models: Vec<ModelInfo>) -> impl Iterator<Item = ModelInfo> {
    let provider = section.name;
    models.into_iter().map(move |model| ModelInfo {
        provider: provider.to_string(),
        ..model
    })
}

/// A provider's error as its section's: its field under the section.
fn of_section(name: &str, error: ModuleError) -> ModuleError {
    let field = error.field.as_ref().map(|f| format!("{name}.{f}"));
    let named = ModuleError::new(error.kind, format!("{name}: {}", error.message));
    match field {
        Some(field) => named.with_field(field),
        None => named,
    }
}

#[async_trait]
impl ModelProvider for Joined {
    fn models(&self) -> Vec<ModelInfo> {
        self.on()
            .into_iter()
            .flat_map(|s| named(s, s.provider.models()))
            .collect()
    }

    /// Every provider's listing once it is in.
    ///
    /// # Errors
    /// The first provider whose listing was refused (its key, as its
    /// section's field).
    async fn listed(&self) -> Result<Vec<ModelInfo>, ModuleError> {
        let mut models = Vec::new();
        for section in self.on() {
            let listed = section
                .provider
                .listed()
                .await
                .map_err(|e| of_section(section.name, e))?;
            models.extend(named(section, listed));
        }
        Ok(models)
    }

    async fn generate(
        &self,
        scope: &Scope,
        request: ModelRequest,
    ) -> Result<ModelRun, ModelFailure> {
        let provider = self.serving(&request.model).await.ok_or_else(|| {
            ModelFailure::new(
                ModelFailureKind::Invalid,
                format!("no provider with a key serves the model {}", request.model),
            )
        })?;
        provider.generate(scope, request).await
    }
}

#[cfg(test)]
mod tests;
