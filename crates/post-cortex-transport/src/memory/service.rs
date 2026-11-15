// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! The `pcx.v1` service's state: the memory it serves, the build `Health`
//! names and the running modules' document kinds.

use std::sync::Arc;

use post_cortex_memory::Memory;
use post_cortex_ports::DocumentKinds;
use post_cortex_proto::pb::HealthResponse;
use post_cortex_proto::pb::post_cortex_server::PostCortexServer;

/// What `Health` names the server as: the build it runs, which only the
/// binary knows.
#[derive(Debug, Clone, Copy)]
pub struct Build {
    /// The server's version.
    pub version: &'static str,
    /// The commit it is built from (abbreviated); empty when unknown.
    pub commit: &'static str,
}

/// gRPC service over the capability services: each handler translates the
/// proto request, calls one service method and maps the result back.
#[derive(Clone)]
pub struct PcxGrpcService {
    memory: Arc<Memory>,
    build: Build,
    kinds: Arc<dyn DocumentKinds>,
}

impl PcxGrpcService {
    /// The service over the capability services the other transports use,
    /// so gRPC, MCP and REST share one write pipeline and redactor and
    /// their writes are ordered together; `kinds` are the running
    /// modules' document kinds.
    pub fn new(memory: Arc<Memory>, build: Build, kinds: Arc<dyn DocumentKinds>) -> Self {
        Self {
            memory,
            build,
            kinds,
        }
    }

    /// The tonic server of the service.
    pub fn into_server(self) -> PostCortexServer<Self> {
        PostCortexServer::new(self)
    }

    /// The memory it serves.
    pub fn memory(&self) -> &Memory {
        &self.memory
    }

    /// The running modules' document kinds.
    pub fn kinds(&self) -> &dyn DocumentKinds {
        self.kinds.as_ref()
    }

    /// The server's health, as `Health` answers it.
    pub fn health_response(&self) -> HealthResponse {
        let health = self.memory.admin.health();
        HealthResponse {
            healthy: health.healthy,
            version: self.build.version.to_string(),
            commit: self.build.commit.to_string(),
            active_sessions: health.active_sessions as u64,
            // Deprecated field, kept for older clients: the same count.
            total_updates: health.total_requests,
            embeddings_enabled: health.embeddings_enabled,
            total_requests: health.total_requests,
        }
    }
}
