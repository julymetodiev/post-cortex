// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Discord's Gateway over a WebSocket: frames of JSON text, read into
//! their envelope and written from an operation and what it carries.
//! What a frame means is the domain's.

use std::sync::Arc;

use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use pcx_module_api::{ModuleError, ModuleErrorKind};
use rustls::{ClientConfig, RootCertStore};
use serde_json::json;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{Connector, MaybeTlsStream, WebSocketStream};

use crate::domain::gateway::{Frame, Sent};
use crate::ports::{Gateway, Link};

/// The Gateway's version and encoding, asked in the URL.
const QUERY: &str = "?v=10&encoding=json";

fn unavailable(what: impl std::fmt::Display) -> ModuleError {
    ModuleError::new(
        ModuleErrorKind::Unavailable,
        format!("Discord's Gateway: {what}"),
    )
}

/// The Gateway's TLS: the public certificate authorities, with rustls'
/// `ring` provider named, since the process sets no default one (as
/// pcx-kube names it).
fn tls() -> Result<Connector, ModuleError> {
    let roots = RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let config =
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(|e| unavailable(format!("its TLS could not be set up: {e}")))?
            .with_root_certificates(roots)
            .with_no_client_auth();
    Ok(Connector::Rustls(Arc::new(config)))
}

/// Discord's Gateway.
pub struct WebSocketGateway;

#[async_trait]
impl Gateway for WebSocketGateway {
    async fn connect(&self, url: &str) -> Result<Box<dyn Link>, ModuleError> {
        let url = format!("{}/{QUERY}", url.trim_end_matches('/'));
        let (socket, _) =
            tokio_tungstenite::connect_async_tls_with_config(&url, None, false, Some(tls()?))
                .await
                .map_err(|e| unavailable(format!("no connection: {e}")))?;
        Ok(Box::new(Socket { socket }))
    }
}

/// One connection.
struct Socket {
    socket: WebSocketStream<MaybeTlsStream<TcpStream>>,
}

#[async_trait]
impl Link for Socket {
    async fn next(&mut self) -> Result<Option<Frame>, ModuleError> {
        loop {
            let Some(message) = self.socket.next().await else {
                return Ok(None);
            };
            match message.map_err(|e| unavailable(format!("the connection broke: {e}")))? {
                Message::Text(text) => {
                    return serde_json::from_str(text.as_str())
                        .map(Some)
                        .map_err(|e| unavailable(format!("a frame that is not its JSON: {e}")));
                }
                Message::Close(_) => return Ok(None),
                // The socket answers a ping itself; nothing else is the
                // Gateway's with this encoding.
                Message::Ping(_) | Message::Pong(_) | Message::Binary(_) | Message::Frame(_) => {}
            }
        }
    }

    async fn send(&mut self, sent: &Sent) -> Result<(), ModuleError> {
        let frame = json!({ "op": sent.op, "d": sent.d }).to_string();
        self.socket
            .send(Message::Text(frame.into()))
            .await
            .map_err(|e| unavailable(format!("the connection broke: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::rest::HttpRest;
    use crate::domain::API_URL;
    use crate::domain::gateway::{Next, Session};
    use crate::ports::Rest;

    /// Against Discord itself, with the bot of `DISCORD_BOT_TOKEN`: the API
    /// names the Gateway, which says hello and, identified, makes a session.
    #[tokio::test]
    #[ignore = "needs DISCORD_BOT_TOKEN and the network"]
    async fn live_the_gateway_makes_a_session_for_the_bot() {
        let token = std::env::var("DISCORD_BOT_TOKEN").unwrap();
        let rest = HttpRest::new(API_URL, &token).unwrap();
        let me = rest.me().await.unwrap();
        assert!(me.bot, "the token is a bot's");
        let url = rest.gateway().await.unwrap();
        let mut link = WebSocketGateway.connect(&url).await.unwrap();
        let mut session = Session::default();
        while session.resumed_at().is_none() {
            let frame = link.next().await.unwrap().expect("the Gateway closed");
            match session.on(&token, frame).unwrap() {
                Next::Begin { send, .. } | Next::Send(send) => link.send(&send).await.unwrap(),
                Next::Reconnect => panic!("the Gateway refused the session"),
                Next::Nothing | Next::Event(..) => {}
            }
        }
    }
}
