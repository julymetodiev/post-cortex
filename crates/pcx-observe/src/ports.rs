// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What observe needs of the world (ARCHITECTURE section 8, operations):
//! an environment's HTTP APIs, each answer's body as it came, and the
//! time. The module's adapter reaches them with the credentials its files
//! hold; tests answer in memory.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use pcx_module_api::ModuleError;
use post_cortex_proto::observe::Environment;

/// Which API of an environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Api {
    /// Prometheus, Loki or Alertmanager, with their token.
    Monitoring,
    /// The Kubernetes API, with the cluster's token and CA.
    Kube,
    /// Nomad's API, with its token and CA.
    Nomad,
}

/// An environment's APIs.
#[async_trait]
pub trait Apis: Send + Sync {
    /// GET `url` (with `query`) of `environment`'s `api`: the body, and
    /// whether the status was a success.
    ///
    /// # Errors
    /// Its credentials could not be read, or the API could not be reached.
    async fn get(
        &self,
        environment: &Environment,
        api: Api,
        url: &str,
        query: &[(&str, String)],
    ) -> Result<(bool, Vec<u8>), ModuleError>;
}

/// The time.
pub trait Clock: Send + Sync {
    /// Now.
    fn now(&self) -> DateTime<Utc>;
}
