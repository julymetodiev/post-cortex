// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The running daemon's gRPC API (`pcx.v1`), for the admin commands: while
//! the daemon runs they go through it, and run offline otherwise.

use post_cortex_daemon::daemon::DaemonConfig;
use post_cortex_proto::pb::post_cortex_client::PostCortexClient;
use tonic::metadata::{Ascii, MetadataValue};
use tonic::service::Interceptor;
use tonic::service::interceptor::InterceptedService;
use tonic::transport::{Channel, Endpoint};

/// A client of the running daemon.
pub type Daemon = PostCortexClient<InterceptedService<Channel, Bearer>>;

/// Sends the daemon's bearer token with every call, when it has one.
#[derive(Clone)]
pub struct Bearer(Option<MetadataValue<Ascii>>);

impl Interceptor for Bearer {
    fn call(
        &mut self,
        mut request: tonic::Request<()>,
    ) -> Result<tonic::Request<()>, tonic::Status> {
        if let Some(token) = &self.0 {
            request
                .metadata_mut()
                .insert("authorization", token.clone());
        }
        Ok(request)
    }
}

/// The running daemon's client; none when the daemon does not run.
///
/// # Errors
/// It runs without gRPC (`grpc_port = 0`), its token is not a valid header
/// value, or it cannot be reached.
pub async fn daemon(config: &DaemonConfig) -> Result<Option<Daemon>, String> {
    if !post_cortex_cli::is_daemon_running(&config.host, config.port).await {
        return Ok(None);
    }
    if config.grpc_port == 0 {
        return Err(format!(
            "the daemon at {}:{} runs without gRPC (grpc_port = 0): set grpc_port, \
             or stop it to run this offline",
            config.host, config.port
        ));
    }
    let mut grpc = config.local();
    grpc.port = config.grpc_port;
    let url = grpc.url();
    let channel = Endpoint::from_shared(url.clone())
        .map_err(|e| format!("{url}: {e}"))?
        .connect()
        .await
        .map_err(|e| format!("cannot reach the daemon's gRPC at {url}: {e}"))?;
    let token = config
        .auth_token
        .as_deref()
        .map(|t| format!("Bearer {}", t.trim()).parse())
        .transpose()
        .map_err(|_| "the daemon's auth_token is not a valid header value".to_string())?;
    Ok(Some(PostCortexClient::with_interceptor(
        channel,
        Bearer(token),
    )))
}
