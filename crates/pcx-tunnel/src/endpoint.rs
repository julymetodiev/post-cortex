//! The gRPC channel a daemon reaches the cloud over.

use std::time::Duration;

use tonic::transport::{Channel, ClientTlsConfig, Endpoint};

use crate::TunnelError;

/// A channel to the gateway at `url`, over TLS for `https`.
pub async fn channel(url: &str) -> Result<Channel, TunnelError> {
    let mut endpoint = Endpoint::from_shared(url.to_owned())?
        .connect_timeout(Duration::from_secs(10))
        .http2_keep_alive_interval(Duration::from_secs(30))
        .keep_alive_while_idle(true);
    if url.starts_with("https://") {
        endpoint = endpoint.tls_config(ClientTlsConfig::new().with_native_roots())?;
    }
    Ok(endpoint.connect().await?)
}
