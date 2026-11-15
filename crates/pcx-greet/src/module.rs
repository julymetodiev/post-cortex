// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The greet module: its manifest (its service, descriptors, RPCs and
//! views), its configuration, its HTML card and its service.

use std::sync::Arc;

use async_trait::async_trait;
use pcx_module_api::grpc::{self, GrpcChannel};
use pcx_module_api::view::HTML_MIME;
use pcx_module_api::{
    API_VERSION, Capability, Effect, Event, HostContext, Manifest, Module, ModuleError,
    ModuleErrorKind, ModuleKind, Outcome, Resource, Rpc, Scope,
};

use crate::pb::GreetConfig;
use crate::pb::greeter_server::GreeterServer;
use crate::service::{Service, State};
use crate::view::{CARD, CARD_HTML, card_view, greetings_view};
use crate::{DESCRIPTORS, ID};

/// The message of its configuration.
const CONFIG: &str = "example.greet.v1.GreetConfig";

/// The module's service.
pub const SERVICE: &str = "example.greet.v1.Greeter";

/// The gRPC path of `method` of the service.
pub fn path(method: &str) -> String {
    format!("/{SERVICE}/{method}")
}

/// Its RPCs: `Greet` a tool (`greet`) for agents and people, the others
/// no tool, for its views; greeting and forgetting write its greetings.
fn rpcs() -> Vec<Rpc> {
    [
        ("Greet", "greet", Effect::Write),
        ("ListGreetings", "", Effect::Read),
        ("Forget", "", Effect::Write),
    ]
    .map(|(method, tool, effect)| Rpc {
        method: path(method),
        tool: tool.to_string(),
        visibility: Vec::new(),
        effect: effect.into(),
    })
    .to_vec()
}

/// The greet module.
pub struct Greet {
    manifest: Manifest,
    state: Arc<State>,
}

impl Greet {
    /// The module, not started.
    pub fn new() -> Self {
        Self {
            manifest: Manifest {
                description:
                    "Answers its greet tool with the configured greeting: the example module."
                        .to_string(),
                id: ID.to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                api: Some(API_VERSION),
                kind: ModuleKind::Tool.into(),
                capabilities: vec![Capability::UiHtml.into()],
                config_type: CONFIG.to_string(),
                views: vec![greetings_view(), card_view()],
                services: vec![SERVICE.to_string()],
                descriptors: DESCRIPTORS.to_vec(),
                rpcs: rpcs(),
                ..Manifest::default()
            },
            state: Arc::default(),
        }
    }
}

impl Default for Greet {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Module for Greet {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    async fn start(&self, ctx: HostContext) -> Result<(), ModuleError> {
        let config: GreetConfig = ctx.config()?;
        if config.greeting.is_empty() {
            return Err(ModuleError::new(
                ModuleErrorKind::InvalidArgument,
                "the configuration needs a greeting, like Hello",
            )
            .with_field("greeting"));
        }
        *self.state.greeting.write().await = config.greeting;
        *self.state.signature.write().await = config.signature;
        Ok(())
    }

    async fn on_event(&self, _: &Scope, _: &Event) -> Result<Outcome, ModuleError> {
        Ok(Outcome::default())
    }

    async fn resource(&self, uri: &str) -> Result<Resource, ModuleError> {
        if uri != CARD {
            return Err(ModuleError::new(
                ModuleErrorKind::NotFound,
                format!("no resource {uri}"),
            ));
        }
        Ok(Resource {
            uri: CARD.to_string(),
            mime_type: HTML_MIME.to_string(),
            text: CARD_HTML.to_string(),
        })
    }

    async fn stop(&self) -> Result<(), ModuleError> {
        Ok(())
    }

    fn services(&self) -> Option<GrpcChannel> {
        Some(grpc::routes(tonic::service::Routes::new(
            GreeterServer::new(Service(Arc::clone(&self.state))),
        )))
    }
}
