//! The gateway's side: answer a daemon's `Serve` call and hold its
//! [`Link`], over which the gateway forwards requests (`docs/connect.md`
//! section 3). Who the daemon is, and whether it may connect, the gateway
//! decides before and in its `admit`.

use std::fmt::Display;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use bytes::Bytes;
use futures::StreamExt;
use http::{Request, Response, StatusCode};
use http_body::Body;
use post_cortex_proto::tunnel::{
    DaemonFrame, GatewayFrame, Hello, Open, Welcome, daemon_frame, gateway_frame,
};
use tokio::sync::{Semaphore, mpsc};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tokio_util::sync::CancellationToken;
use tonic::Status;
use tracing::{debug, info};

use crate::TunnelError;
use crate::exchanges::{Exchanges, Incoming, Unread, send_body};
use crate::frame::{FromDaemon, from_wire, to_wire};
use crate::session;

/// The gateway's frames to the daemon, the answer of its `Serve` call.
pub type Frames = UnboundedReceiverStream<Result<GatewayFrame, Status>>;

/// The body of a daemon's answer.
pub type AnswerBody = Incoming<Result<GatewayFrame, Status>>;

/// How much a tunnel carries at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// The body bytes of an exchange sent before a grant.
    pub window: u32,
    /// The exchanges open at once at most.
    pub max_exchanges: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            window: crate::WINDOW,
            max_exchanges: crate::MAX_EXCHANGES,
        }
    }
}

/// A connected daemon's tunnel, as the gateway holds it.
#[derive(Clone)]
pub struct Link {
    exchanges: Arc<Exchanges<Result<GatewayFrame, Status>>>,
    next: Arc<AtomicU64>,
    slots: Arc<Semaphore>,
    max_exchanges: u32,
    closed: CancellationToken,
}

impl Link {
    /// Forward `request` to the daemon: its answer, whose body streams
    /// back as the daemon sends it. Only the path and query of its URI
    /// travel.
    ///
    /// # Errors
    /// [`TunnelError::Busy`] with too many exchanges open, [`TunnelError::Closed`]
    /// once the tunnel is, [`TunnelError::Reset`] when the daemon ended the
    /// exchange, [`TunnelError::Http`] for an answer HTTP does not allow.
    pub async fn send<B>(&self, request: Request<B>) -> Result<Response<AnswerBody>, TunnelError>
    where
        B: Body<Data = Bytes> + Send + 'static,
        B::Error: Display,
    {
        let Ok(slot) = Arc::clone(&self.slots).try_acquire_owned() else {
            return Err(TunnelError::Busy(self.max_exchanges));
        };
        if self.closed.is_cancelled() {
            return Err(TunnelError::Closed);
        }
        let exchange = self.next.fetch_add(1, Ordering::Relaxed);
        let (answer, credit) = self.exchanges.open(exchange, Some(slot))?;
        let head = self.exchanges.expect_head(exchange);
        let (parts, body) = request.into_parts();
        let open = Open {
            exchange,
            method: parts.method.as_str().to_owned(),
            path: parts
                .uri
                .path_and_query()
                .map_or_else(|| "/".to_owned(), |p| p.as_str().to_owned()),
            headers: to_wire(&parts.headers),
        };
        if !self.exchanges.send_frame(Ok(GatewayFrame {
            frame: Some(gateway_frame::Frame::Open(open)),
        })) {
            return Err(TunnelError::Closed);
        }
        let task = tokio::spawn(send_body(
            Arc::clone(&self.exchanges),
            exchange,
            credit,
            body,
        ));
        self.exchanges.track(exchange, task.abort_handle());
        let head = match head.await {
            Ok(head) => head?,
            Err(_) => return Err(TunnelError::Closed),
        };
        let status = u16::try_from(head.status)
            .ok()
            .and_then(|s| StatusCode::from_u16(s).ok())
            .ok_or_else(|| TunnelError::Http(format!("status {}", head.status)))?;
        let mut response = Response::new(answer);
        *response.status_mut() = status;
        *response.headers_mut() = from_wire(head.headers)?;
        Ok(response)
    }

    /// Whether `other` is this same tunnel.
    #[must_use]
    pub fn same(&self, other: &Link) -> bool {
        Arc::ptr_eq(&self.exchanges, &other.exchanges)
    }

    /// Close the tunnel (the daemon was revoked): its exchanges fail.
    pub fn close(&self) {
        self.closed.cancel();
    }

    /// Resolves once the tunnel has ended.
    pub async fn closed(&self) {
        self.closed.cancelled().await;
    }
}

/// Answer a daemon's `Serve` call: its first frame is its `Hello`, which
/// `admit` answers with the daemon's `Welcome` (its window and most
/// exchanges are set from `limits`) or refuses, keeping the daemon's
/// [`Link`] to forward requests over until it closes.
pub fn serve<S, A, F>(inbound: S, limits: Limits, admit: A) -> Frames
where
    S: futures::Stream<Item = Result<DaemonFrame, Status>> + Unpin + Send + 'static,
    A: FnOnce(Hello, Link) -> F + Send + 'static,
    F: Future<Output = Result<Welcome, Status>> + Send + 'static,
{
    let (out, frames) = mpsc::unbounded_channel();
    tokio::spawn(run(inbound, out, limits, admit));
    UnboundedReceiverStream::new(frames)
}

async fn run<S, A, F>(
    mut inbound: S,
    out: mpsc::UnboundedSender<Result<GatewayFrame, Status>>,
    limits: Limits,
    admit: A,
) where
    S: futures::Stream<Item = Result<DaemonFrame, Status>> + Unpin,
    A: FnOnce(Hello, Link) -> F,
    F: Future<Output = Result<Welcome, Status>>,
{
    let hello = match inbound.next().await.map(|f| f.map(|f| f.frame)) {
        Some(Ok(Some(daemon_frame::Frame::Hello(hello)))) => hello,
        Some(Err(status)) => return debug!("a daemon's call failed before its Hello: {status}"),
        _ => {
            if out
                .send(Err(Status::invalid_argument("the first frame is a Hello")))
                .is_err()
            {
                debug!("a daemon left before its Hello");
            }
            return;
        }
    };
    let exchanges = Exchanges::new(out, limits.window, Unread::Reset);
    let link = Link {
        exchanges: Arc::clone(&exchanges),
        next: Arc::new(AtomicU64::new(1)),
        slots: Arc::new(Semaphore::new(limits.max_exchanges as usize)),
        max_exchanges: limits.max_exchanges,
        closed: CancellationToken::new(),
    };
    let welcome = match admit(hello, link.clone()).await {
        Ok(welcome) => Welcome {
            window: limits.window,
            max_exchanges: limits.max_exchanges,
            ..welcome
        },
        Err(status) => {
            exchanges.send_frame(Err(status));
            exchanges.close();
            link.closed.cancel();
            return;
        }
    };
    let daemon = welcome.daemon_id.clone();
    exchanges.send_frame(Ok(GatewayFrame {
        frame: Some(gateway_frame::Frame::Welcome(welcome)),
    }));
    let heads = Arc::clone(&exchanges);
    let result = session::read(
        inbound,
        Arc::clone(&exchanges),
        |frame| match frame {
            FromDaemon::Head(head) => {
                heads.head(head);
                Ok(())
            }
            FromDaemon::Hello => Err(TunnelError::Protocol("a second Hello".into())),
        },
        link.closed.clone(),
    )
    .await;
    match result {
        Ok(()) => info!(daemon, "a daemon's tunnel was closed"),
        Err(e) => info!(daemon, error = %e, "a daemon's tunnel ended"),
    }
    exchanges.close();
    link.closed.cancel();
}
