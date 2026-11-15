// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The `observe` module: its manifest (its configuration and its service,
//! each RPC an agent's tool) and its service over its environments' APIs.

use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use pcx_module_api::grpc::{self, GrpcChannel};
use pcx_module_api::{
    API_VERSION, Capability, Event, HostContext, Manifest, Module, ModuleError, ModuleErrorKind,
    ModuleKind, Outcome, Resource, Scope,
};
use post_cortex_proto::module::{Effect, Rpc};
use post_cortex_proto::observe::ObserveConfig;
use post_cortex_proto::observe::observe_server::ObserveServer;
use prost::Name;

use crate::adapters::HttpApis;
use crate::domain::MODULE_ID;
use crate::handlers::Handlers;
use crate::service;

/// The module's service.
const SERVICE: &str = "pcx.observe.v1.Observe";

/// Each RPC of the service and its tool name: all an agent may call.
const RPCS: [(&str, &str, Effect); 8] = [
    ("QueryMetrics", "metrics_query", Effect::Read),
    ("QueryLogs", "logs_query", Effect::Read),
    ("ListAlerts", "alerts_list", Effect::Read),
    ("GetCluster", "cluster_get", Effect::Read),
    ("PodLog", "pod_log", Effect::Read),
    ("GetJobs", "jobs_get", Effect::Read),
    ("AllocLog", "alloc_log", Effect::Read),
    ("GetNodes", "node_get", Effect::Read),
];

/// What a started module works with.
pub struct Running {
    /// Its configuration, checked.
    pub config: ObserveConfig,
    /// Its environments' APIs.
    pub apis: HttpApis,
}

/// The running module, shared by the module and its service; none until
/// it starts.
pub type Shared = Arc<Mutex<Option<Arc<Running>>>>;

/// The running module of `shared`.
///
/// # Errors
/// `Unavailable` before it starts.
pub fn running(shared: &Shared) -> Result<Arc<Running>, ModuleError> {
    shared
        .lock()
        .clone()
        .ok_or_else(|| ModuleError::new(ModuleErrorKind::Unavailable, "observe has not started"))
}

/// The `observe` module.
pub struct ObserveModule {
    manifest: Manifest,
    running: Shared,
}

impl ObserveModule {
    /// The module's id.
    pub const ID: &'static str = MODULE_ID;

    /// The module.
    #[must_use]
    pub fn new() -> Self {
        Self {
            manifest: Manifest {
                description: "Reads a running system, never changes it: metrics (Prometheus), logs (Loki), alerts (Alertmanager) and a cluster (Kubernetes, or Nomad: jobs, allocations, nodes), per environment.".to_string(),
                id: MODULE_ID.to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                api: Some(API_VERSION),
                kind: ModuleKind::Tool.into(),
                capabilities: vec![Capability::Network.into()],
                config_type: ObserveConfig::full_name(),
                services: vec![SERVICE.to_string()],
                descriptors: post_cortex_proto::OBSERVE_DESCRIPTOR_SET.to_vec(),
                rpcs: RPCS
                    .iter()
                    .map(|(method, tool, effect)| Rpc {
                        method: format!("/{SERVICE}/{method}"),
                        tool: (*tool).to_string(),
                        visibility: Vec::new(),
                        effect: (*effect).into(),
                    })
                    .collect(),
                ..Manifest::default()
            },
            running: Shared::default(),
        }
    }
}

impl Default for ObserveModule {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Module for ObserveModule {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// The host has checked the configuration against its message.
    async fn start(&self, ctx: HostContext) -> Result<(), ModuleError> {
        let config = service::checked(ctx.config()?)?;
        *self.running.lock() = Some(Arc::new(Running {
            config,
            apis: HttpApis::new(ctx.host),
        }));
        Ok(())
    }

    /// Its `pcx.observe.v1.Observe` service.
    fn services(&self) -> Option<GrpcChannel> {
        let handlers = Handlers {
            shared: Arc::clone(&self.running),
        };
        Some(grpc::routes(tonic::service::Routes::new(
            ObserveServer::new(handlers),
        )))
    }

    async fn on_event(&self, _scope: &Scope, _event: &Event) -> Result<Outcome, ModuleError> {
        Ok(Outcome::default())
    }

    async fn resource(&self, uri: &str) -> Result<Resource, ModuleError> {
        Err(ModuleError::new(
            ModuleErrorKind::NotFound,
            format!("observe serves no resource {uri}"),
        ))
    }

    async fn stop(&self) -> Result<(), ModuleError> {
        *self.running.lock() = None;
        Ok(())
    }
}
