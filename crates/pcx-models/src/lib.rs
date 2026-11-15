// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The models module (docs/modules/agent.md, the providers; owner
//! 2026-11-09: every provider in one module, not a module for each): of
//! kind `model_provider`, with the `network` grant. Its settings have a
//! section for each provider; a provider is on when its API key is set,
//! its models listed with the others', each carrying its provider's name.

#![forbid(unsafe_code)]

mod joined;

use std::sync::Arc;

use async_trait::async_trait;
use pcx_module_api::model::ModelProvider;
use pcx_module_api::{
    API_VERSION, Capability, Event, HostContext, Manifest, Module, ModuleError, ModuleErrorKind,
    ModuleKind, Outcome, Resource, Scope,
};
use pcx_provider_anthropic::Anthropic;
use post_cortex_proto::models::ModelsConfig;
use prost::Name;

use crate::joined::{Joined, Section};

/// Every model provider, each on with its API key.
pub struct ModelsModule {
    manifest: Manifest,
    joined: Arc<Joined>,
}

impl ModelsModule {
    /// The module's id.
    pub const ID: &'static str = "models";

    /// The module, every provider off until it starts.
    #[must_use]
    pub fn new() -> Self {
        // In the order of its settings, which is the order a model's
        // provider is looked for in.
        let sections = vec![
            Section {
                name: pcx_provider_openrouter::ID,
                endpoint: pcx_provider_openrouter::ENDPOINT,
                keyed: |c| c.openrouter.as_ref(),
                provider: Arc::new(pcx_provider_openrouter::provider()),
            },
            Section {
                name: pcx_provider_anthropic::ID,
                endpoint: pcx_provider_anthropic::ENDPOINT,
                keyed: |c| c.anthropic.as_ref(),
                provider: Arc::new(Anthropic::default()),
            },
            Section {
                name: pcx_provider_zai::ID,
                endpoint: pcx_provider_zai::ENDPOINT,
                keyed: |c| c.zai.as_ref(),
                provider: Arc::new(pcx_provider_zai::provider()),
            },
            Section {
                name: pcx_provider_openai::ID,
                endpoint: pcx_provider_openai::ENDPOINT,
                keyed: |c| c.openai.as_ref(),
                provider: Arc::new(pcx_provider_openai::provider()),
            },
        ];
        let manifest = Manifest {
            description: "Every model of OpenRouter, Anthropic, Z.AI and OpenAI, each provider on with its API key.".to_string(),
            id: Self::ID.to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            api: Some(API_VERSION),
            kind: ModuleKind::ModelProvider.into(),
            capabilities: vec![Capability::Network.into()],
            descriptors: post_cortex_proto::MODELS_DESCRIPTOR_SET.to_vec(),
            config_type: ModelsConfig::full_name(),
            ..Manifest::default()
        };
        Self {
            manifest,
            joined: Arc::new(Joined::new(sections)),
        }
    }
}

impl Default for ModelsModule {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Module for ModelsModule {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// The host has checked the configuration against its message.
    async fn start(&self, ctx: HostContext) -> Result<(), ModuleError> {
        let config: ModelsConfig = ctx.config()?;
        self.joined.start(&config).await
    }

    async fn on_event(&self, _: &Scope, _: &Event) -> Result<Outcome, ModuleError> {
        Ok(Outcome::default())
    }

    async fn resource(&self, uri: &str) -> Result<Resource, ModuleError> {
        Err(ModuleError::new(
            ModuleErrorKind::NotFound,
            format!("{} serves no resource {uri}", Self::ID),
        ))
    }

    async fn stop(&self) -> Result<(), ModuleError> {
        self.joined.stop();
        Ok(())
    }

    fn model_provider(&self) -> Option<Arc<dyn ModelProvider>> {
        let provider: Arc<dyn ModelProvider> = Arc::clone(&self.joined) as Arc<dyn ModelProvider>;
        Some(provider)
    }
}
