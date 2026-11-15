//! The daemon's side: connect to the gateway, say hello, and serve each
//! request the gateway forwards with a router, until the daemon stops;
//! connect again when the tunnel breaks (`docs/connect.md` section 4).

use std::convert::Infallible;
use std::fmt::Display;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use futures::StreamExt;
use http::{Method, Request, Response, Uri, Version};
use http_body::Body;
use post_cortex_proto::tunnel::tunnel_client::TunnelClient;
use post_cortex_proto::tunnel::{
    DaemonFrame, GatewayFrame, Head, Hello, LeaveRequest, Open, Welcome, daemon_frame,
};
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tokio_util::sync::CancellationToken;
use tower::{Service, ServiceExt};
use tracing::{error, info, warn};

use crate::exchanges::{Exchanges, Incoming, Unread, send_body};
use crate::frame::{FromGateway, Inbound, Received, from_wire, to_wire};
use crate::session;
use crate::{TunnelError, endpoint};

/// The body of a request the gateway forwards.
pub type RequestBody = Incoming<DaemonFrame>;

/// The first wait before connecting again; it doubles up to [`MAX_BACKOFF`].
const MIN_BACKOFF: Duration = Duration::from_millis(500);

/// The longest wait before connecting again.
const MAX_BACKOFF: Duration = Duration::from_secs(60);

/// A tunnel that lived this long was not failing: the next wait starts
/// from [`MIN_BACKOFF`] again.
const STABLE: Duration = Duration::from_secs(60);

/// Where, and as which daemon, to connect.
#[derive(Debug, Clone)]
pub struct Connection {
    /// The gateway's URL (`https://...`).
    pub url: String,
    /// The daemon's token (`pcxd_...`).
    pub token: String,
}

/// Serve the tunnel of `connection` until `ct` is canceled, connecting
/// again with backoff when it breaks; at each connection `hello` says what
/// the daemon offers, and `serve` gives the service of the gateway's
/// `Welcome` (what the space offers now). Returns early when the gateway
/// refuses the token (revoked: connecting again would not help).
pub async fn run<M, S, B, H, F>(connection: Connection, serve: M, hello: H, ct: CancellationToken)
where
    M: Fn(&Welcome) -> S,
    S: Service<Request<RequestBody>, Response = Response<B>, Error = Infallible>
        + Clone
        + Send
        + 'static,
    S::Future: Send,
    B: Body<Data = Bytes> + Send + 'static,
    B::Error: Display,
    H: Fn() -> F,
    F: Future<Output = Hello>,
{
    let mut backoff = MIN_BACKOFF;
    loop {
        let started = Instant::now();
        let hello = hello().await;
        match serve_once(&connection, &serve, hello, &ct).await {
            Ok(()) => return,
            Err(TunnelError::Status(status)) if status.code() == tonic::Code::Unauthenticated => {
                error!(
                    "the cloud refused this daemon's token ({}): run `pcx connect` again",
                    status.message()
                );
                return;
            }
            Err(e) => warn!(error = %e, "the tunnel to the cloud broke"),
        }
        if started.elapsed() > STABLE {
            backoff = MIN_BACKOFF;
        }
        let wait = jitter(backoff);
        backoff = (backoff * 2).min(MAX_BACKOFF);
        tokio::select! {
            () = ct.cancelled() => return,
            () = tokio::time::sleep(wait) => {}
        }
    }
}

/// Tell the cloud this daemon leaves its organisation (`pcx connect
/// --off`): its token no longer connects.
///
/// # Errors
/// The gateway could not be reached or refused.
pub async fn leave(connection: &Connection) -> Result<(), TunnelError> {
    let mut client = TunnelClient::new(endpoint::channel(&connection.url).await?);
    let mut request = tonic::Request::new(LeaveRequest {});
    authorize(&mut request, &connection.token)?;
    client.leave(request).await?;
    Ok(())
}

/// One tunnel, from connecting to its end: `Ok` when `ct` ends it.
async fn serve_once<M, S, B>(
    connection: &Connection,
    serve: &M,
    hello: Hello,
    ct: &CancellationToken,
) -> Result<(), TunnelError>
where
    M: Fn(&Welcome) -> S,
    S: Service<Request<RequestBody>, Response = Response<B>, Error = Infallible>
        + Clone
        + Send
        + 'static,
    S::Future: Send,
    B: Body<Data = Bytes> + Send + 'static,
    B::Error: Display,
{
    let mut client = TunnelClient::new(endpoint::channel(&connection.url).await?);
    let (out, frames) = mpsc::unbounded_channel();
    if out
        .send(DaemonFrame {
            frame: Some(daemon_frame::Frame::Hello(hello)),
        })
        .is_err()
    {
        return Err(TunnelError::Closed);
    }
    let mut request = tonic::Request::new(UnboundedReceiverStream::new(frames));
    authorize(&mut request, &connection.token)?;
    let mut inbound = client.serve(request).await?.into_inner();
    let welcome = welcome(inbound.next().await)?;
    info!(
        daemon = %welcome.daemon_id,
        workspaces = welcome.workspaces.len(),
        "connected to the cloud"
    );
    let service = serve(&welcome);
    let exchanges = Exchanges::new(out, welcome.window, Unread::Drain);
    let opened = Arc::clone(&exchanges);
    let result = session::read(
        inbound,
        Arc::clone(&exchanges),
        |frame| match frame {
            FromGateway::Open(open) => {
                start(&opened, &service, open);
                Ok(())
            }
            FromGateway::Welcome(_) => Err(TunnelError::Protocol("a second Welcome".into())),
        },
        ct.clone(),
    )
    .await;
    exchanges.close();
    result
}

/// The gateway's first frame, its `Welcome`.
fn welcome(first: Option<Result<GatewayFrame, tonic::Status>>) -> Result<Welcome, TunnelError> {
    match first {
        None => Err(TunnelError::Closed),
        Some(Err(status)) => Err(status.into()),
        Some(Ok(frame)) => match frame.received() {
            Some(Received::Own(FromGateway::Welcome(welcome))) => Ok(welcome),
            _ => Err(TunnelError::Protocol(
                "the first frame is no Welcome".into(),
            )),
        },
    }
}

/// Serve the request `open` begins: the router's answer goes back as it
/// comes; a request HTTP does not allow is reset.
fn start<S, B>(exchanges: &Arc<Exchanges<DaemonFrame>>, service: &S, open: Open)
where
    S: Service<Request<RequestBody>, Response = Response<B>, Error = Infallible>
        + Clone
        + Send
        + 'static,
    S::Future: Send,
    B: Body<Data = Bytes> + Send + 'static,
    B::Error: Display,
{
    let exchange = open.exchange;
    let parts = Method::from_bytes(open.method.as_bytes())
        .map_err(|e| TunnelError::Http(format!("method {}: {e}", open.method)))
        .and_then(|method| {
            let uri = open
                .path
                .parse::<Uri>()
                .map_err(|e| TunnelError::Http(format!("path {}: {e}", open.path)))?;
            Ok((method, uri, from_wire(open.headers)?))
        });
    let (method, uri, headers) = match parts {
        Ok(parts) => parts,
        Err(e) => return exchanges.reset(exchange, &e.to_string(), true),
    };
    let (body, credit) = match exchanges.open(exchange, None) {
        Ok(opened) => opened,
        Err(e) => return exchanges.reset(exchange, &e.to_string(), true),
    };
    let mut request = Request::new(body);
    // An exchange is an HTTP/2 stream: trailers end its answer (gRPC
    // refuses a call of another version).
    *request.version_mut() = Version::HTTP_2;
    *request.method_mut() = method;
    *request.uri_mut() = uri;
    *request.headers_mut() = headers;
    let answering = Arc::clone(exchanges);
    let service = service.clone();
    let task = tokio::spawn(async move {
        let response = match service.oneshot(request).await {
            Ok(response) => response,
            Err(never) => match never {},
        };
        let (parts, body) = response.into_parts();
        let head = Head {
            exchange,
            status: u32::from(parts.status.as_u16()),
            headers: to_wire(&parts.headers),
        };
        if answering.send_frame(DaemonFrame {
            frame: Some(daemon_frame::Frame::Head(head)),
        }) {
            send_body(answering, exchange, credit, body).await;
        }
    });
    exchanges.track(exchange, task.abort_handle());
}

/// The daemon's token as the call's bearer.
fn authorize<T>(request: &mut tonic::Request<T>, token: &str) -> Result<(), TunnelError> {
    let bearer = format!("Bearer {token}")
        .parse()
        .map_err(|e| TunnelError::Http(format!("the token is no header value: {e}")))?;
    request.metadata_mut().insert("authorization", bearer);
    Ok(())
}

/// `wait` give or take a quarter, so daemons cut off together do not all
/// come back at once.
fn jitter(wait: Duration) -> Duration {
    wait.mul_f64(rand::random_range(0.75..1.25))
}
