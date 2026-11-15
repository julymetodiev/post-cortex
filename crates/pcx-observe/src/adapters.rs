// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Observe's ports: the system clock, and an environment's HTTP APIs with
//! the credentials its files hold, read through the module's sandbox at
//! each call (`pcx-kube`).

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use pcx_kube::{Caller, Clients, Credentials, Method};
use pcx_module_api::{ModuleError, ModuleHost};
use post_cortex_proto::observe::Environment;

use crate::ports::{Api, Apis, Clock};

/// The system's clock.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// The APIs over HTTP, their credentials read through the module's host
/// handle, over clients kept for the next call.
pub struct HttpApis {
    host: Arc<dyn ModuleHost>,
    clients: Clients,
}

impl HttpApis {
    /// The APIs, their credentials read through `host`'s sandbox.
    pub fn new(host: Arc<dyn ModuleHost>) -> Self {
        Self {
            host,
            clients: Clients::default(),
        }
    }
}

#[async_trait]
impl Apis for HttpApis {
    async fn get(
        &self,
        environment: &Environment,
        api: Api,
        url: &str,
        query: &[(&str, String)],
    ) -> Result<(bool, Vec<u8>), ModuleError> {
        let credentials = match api {
            Api::Monitoring => Credentials {
                token_file: &environment.token_file,
                ca_file: "",
                tls_server_name: "",
            },
            Api::Kube => Credentials {
                token_file: &environment.kube_token_file,
                ca_file: &environment.kube_ca_file,
                tls_server_name: &environment.kube_tls_server_name,
            },
            Api::Nomad => Credentials {
                token_file: &environment.nomad_token_file,
                ca_file: &environment.nomad_ca_file,
                tls_server_name: &environment.nomad_tls_server_name,
            },
        };
        let sandbox = self.host.sandbox().await?;
        let answer = Caller::new(sandbox.as_ref(), &self.clients)
            .call(credentials, Method::GET, url, query, None)
            .await?;
        Ok((answer.ok, answer.body))
    }
}
