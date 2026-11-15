// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A module served elsewhere (a runner's, `docs/connect.md` section 7): the
//! host runs it as its own, its services a channel to where it runs. What
//! it may do there (its capabilities, its roots, its configuration) is
//! that place's, so its manifest here keeps only what names its services:
//! it asks this host for no grant, answers no event, keeps no document,
//! calls no module.

use async_trait::async_trait;
use pcx_module_api::grpc::GrpcChannel;
use pcx_module_api::{
    Event, HostContext, Manifest, Module, ModuleError, ModuleErrorKind, Outcome, Resource, Scope,
};

/// A module whose services run elsewhere.
pub struct RemoteModule {
    manifest: Manifest,
    channel: GrpcChannel,
}

impl RemoteModule {
    /// The module of `manifest` (as the place it runs gave it), its calls
    /// sent over `channel`.
    #[must_use]
    pub fn new(manifest: Manifest, channel: GrpcChannel) -> Self {
        Self {
            manifest: Manifest {
                id: manifest.id,
                version: manifest.version,
                api: manifest.api,
                kind: manifest.kind,
                services: manifest.services,
                descriptors: manifest.descriptors,
                rpcs: manifest.rpcs,
                description: manifest.description,
                ..Manifest::default()
            },
            channel,
        }
    }
}

#[async_trait]
impl Module for RemoteModule {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    async fn start(&self, _ctx: HostContext) -> Result<(), ModuleError> {
        Ok(())
    }

    /// Its services, where it runs.
    fn services(&self) -> Option<GrpcChannel> {
        Some(self.channel.clone())
    }

    async fn on_event(&self, _scope: &Scope, _event: &Event) -> Result<Outcome, ModuleError> {
        Ok(Outcome::default())
    }

    async fn resource(&self, uri: &str) -> Result<Resource, ModuleError> {
        Err(ModuleError::new(
            ModuleErrorKind::NotFound,
            format!("{} serves no resource {uri} here", self.manifest.id),
        ))
    }

    async fn stop(&self) -> Result<(), ModuleError> {
        Ok(())
    }
}
