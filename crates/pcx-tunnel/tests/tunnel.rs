//! The tunnel end to end over a real gRPC connection: a gateway serving
//! `Tunnel` on a loopback port, a daemon serving an axum router over it.

use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::routing::{get, post};
use bytes::Bytes;
use http::{Request, StatusCode};
use http_body::Frame;
use http_body_util::{BodyExt, Full, StreamBody};
use pcx_tunnel::TunnelError;
use pcx_tunnel::daemon::{self, Connection};
use pcx_tunnel::gateway::{self, Limits, Link};
use post_cortex_proto::tunnel::tunnel_server::{Tunnel, TunnelServer};
use post_cortex_proto::tunnel::{
    DaemonFrame, Hello, LeaveRequest, Left, Offer, Welcome, Workspace,
};
use tokio::sync::{mpsc, watch};
use tokio_stream::wrappers::TcpListenerStream;
use tokio_util::sync::CancellationToken;

const TOKEN: &str = "pcxd_test";

/// A gateway that admits the daemon with [`TOKEN`] and keeps its link.
struct Gateway {
    link: watch::Sender<Option<Link>>,
    admitted: Arc<AtomicUsize>,
    limits: Limits,
}

#[tonic::async_trait]
impl Tunnel for Gateway {
    type ServeStream = gateway::Frames;

    async fn serve(
        &self,
        request: tonic::Request<tonic::Streaming<DaemonFrame>>,
    ) -> Result<tonic::Response<gateway::Frames>, tonic::Status> {
        let bearer = request
            .metadata()
            .get("authorization")
            .and_then(|v| v.to_str().ok());
        if bearer != Some(format!("Bearer {TOKEN}").as_str()) {
            return Err(tonic::Status::unauthenticated("unknown daemon"));
        }
        let link = self.link.clone();
        let admitted = Arc::clone(&self.admitted);
        let frames = gateway::serve(
            request.into_inner(),
            self.limits,
            move |hello: Hello, l| async move {
                link.send_replace(Some(l));
                admitted.fetch_add(1, Ordering::SeqCst);
                Ok(Welcome {
                    daemon_id: "d1".into(),
                    workspaces: hello.workspaces.into_iter().map(|w| w.id).collect(),
                    ..Welcome::default()
                })
            },
        );
        Ok(tonic::Response::new(frames))
    }

    async fn leave(
        &self,
        _request: tonic::Request<LeaveRequest>,
    ) -> Result<tonic::Response<Left>, tonic::Status> {
        Ok(tonic::Response::new(Left {}))
    }
}

/// A running gateway and daemon.
struct Pair {
    link: watch::Receiver<Option<Link>>,
    admitted: Arc<AtomicUsize>,
    /// The daemon's handlers send here when a hanging call is dropped.
    dropped: mpsc::UnboundedReceiver<()>,
    url: String,
    ct: CancellationToken,
}

impl Pair {
    async fn link(&mut self) -> Link {
        let link = self
            .link
            .wait_for(Option::is_some)
            .await
            .expect("the gateway runs");
        link.clone().expect("a link")
    }
}

impl Drop for Pair {
    fn drop(&mut self) {
        self.ct.cancel();
    }
}

/// Says when the call holding it is dropped.
struct Guard(mpsc::UnboundedSender<()>);

impl Drop for Guard {
    fn drop(&mut self) {
        self.0.send(()).expect("the test listens");
    }
}

fn router(dropped: mpsc::UnboundedSender<()>) -> Router {
    Router::new()
        .route("/echo", post(|body: Bytes| async move { body }))
        .route("/big", get(|| async { vec![7u8; 1024 * 1024] }))
        .route(
            "/trailers",
            get(|| async {
                let mut trailers = http::HeaderMap::new();
                trailers.insert("grpc-status", "0".parse().expect("a value"));
                let frames = futures::stream::iter([
                    Ok::<_, Infallible>(Frame::data(Bytes::from_static(b"answer"))),
                    Ok(Frame::trailers(trailers)),
                ]);
                Body::new(StreamBody::new(frames))
            }),
        )
        .route(
            "/hang",
            get(move || {
                let guard = Guard(dropped.clone());
                async move {
                    std::future::pending::<()>().await;
                    drop(guard);
                }
            }),
        )
        .route("/early", post(|| async { StatusCode::IM_A_TEAPOT }))
}

async fn start(limits: Limits, token: &str) -> Pair {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a port");
    let url = format!("http://{}", listener.local_addr().expect("an address"));
    let (link_tx, link) = watch::channel(None);
    let admitted = Arc::new(AtomicUsize::new(0));
    let gateway = Gateway {
        link: link_tx,
        admitted: Arc::clone(&admitted),
        limits,
    };
    let ct = CancellationToken::new();
    let stop = ct.clone();
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(TunnelServer::new(gateway))
            .serve_with_incoming_shutdown(TcpListenerStream::new(listener), stop.cancelled())
            .await
            .expect("the gateway serves");
    });
    let (dropped_tx, dropped) = mpsc::unbounded_channel();
    let connection = Connection {
        url: url.clone(),
        token: token.to_owned(),
    };
    let routes = router(dropped_tx);
    tokio::spawn(daemon::run(
        connection,
        move |_: &Welcome| routes.clone(),
        hello,
        ct.clone(),
    ));
    Pair {
        link,
        admitted,
        dropped,
        url,
        ct,
    }
}

async fn hello() -> Hello {
    Hello {
        version: "test".into(),
        offers: vec![Offer::Space as i32],
        workspaces: vec![Workspace {
            id: "w1".into(),
            name: "one".into(),
        }],
        lent: Vec::new(),
        roots: Vec::new(),
    }
}

fn request(method: &str, path: &str, body: Bytes) -> Request<Full<Bytes>> {
    Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/octet-stream")
        .body(Full::new(body))
        .expect("a request")
}

#[tokio::test]
async fn a_body_larger_than_the_window_goes_both_ways_whole() {
    let mut pair = start(Limits::default(), TOKEN).await;
    let link = pair.link().await;
    let sent: Bytes = (0..600 * 1024)
        .map(|i| (i % 251) as u8)
        .collect::<Vec<_>>()
        .into();
    let answer = link
        .send(request("POST", "/echo", sent.clone()))
        .await
        .expect("an answer");
    assert_eq!(answer.status(), StatusCode::OK);
    let got = answer
        .into_body()
        .collect()
        .await
        .expect("the body")
        .to_bytes();
    assert_eq!(got, sent);

    let big = link
        .send(request("GET", "/big", Bytes::new()))
        .await
        .expect("an answer");
    let got = big
        .into_body()
        .collect()
        .await
        .expect("the body")
        .to_bytes();
    assert_eq!(got.len(), 1024 * 1024);
    assert!(got.iter().all(|b| *b == 7));
}

#[tokio::test]
async fn an_answers_trailers_come_back() {
    let mut pair = start(Limits::default(), TOKEN).await;
    let link = pair.link().await;
    let answer = link
        .send(request("GET", "/trailers", Bytes::new()))
        .await
        .expect("an answer");
    let collected = answer.into_body().collect().await.expect("the body");
    let trailers = collected.trailers().cloned().expect("trailers");
    assert_eq!(trailers.get("grpc-status").expect("the status"), "0");
    assert_eq!(collected.to_bytes(), Bytes::from_static(b"answer"));
}

#[tokio::test]
async fn an_answer_before_the_request_is_read_ends_well_and_the_tunnel_goes_on() {
    let mut pair = start(Limits::default(), TOKEN).await;
    let link = pair.link().await;
    let answer = link
        .send(request(
            "POST",
            "/early",
            Bytes::from(vec![1u8; 1024 * 1024]),
        ))
        .await
        .expect("an answer");
    assert_eq!(answer.status(), StatusCode::IM_A_TEAPOT);
    answer.into_body().collect().await.expect("the body");
    let echo = link
        .send(request("POST", "/echo", Bytes::from_static(b"again")))
        .await
        .expect("an answer");
    let got = echo
        .into_body()
        .collect()
        .await
        .expect("the body")
        .to_bytes();
    assert_eq!(got, Bytes::from_static(b"again"));
}

#[tokio::test]
async fn a_request_given_up_on_stops_the_daemons_call() {
    let mut pair = start(Limits::default(), TOKEN).await;
    let link = pair.link().await;
    let given_up = tokio::time::timeout(
        Duration::from_millis(200),
        link.send(request("GET", "/hang", Bytes::new())),
    )
    .await;
    assert!(given_up.is_err(), "the call hangs");
    tokio::time::timeout(Duration::from_secs(5), pair.dropped.recv())
        .await
        .expect("the daemon's call stopped")
        .expect("a guard");
}

#[tokio::test]
async fn more_exchanges_than_allowed_are_refused() {
    let limits = Limits {
        max_exchanges: 1,
        ..Limits::default()
    };
    let mut pair = start(limits, TOKEN).await;
    let link = pair.link().await;
    let hanging = link.clone();
    let open =
        tokio::spawn(async move { hanging.send(request("GET", "/hang", Bytes::new())).await });
    tokio::time::sleep(Duration::from_millis(100)).await;
    let refused = link.send(request("GET", "/big", Bytes::new())).await;
    assert!(matches!(refused, Err(TunnelError::Busy(1))), "{refused:?}");
    open.abort();
}

#[tokio::test]
async fn a_refused_token_stops_the_daemon_trying() {
    let pair = start(Limits::default(), "pcxd_wrong").await;
    let connection = Connection {
        url: pair.url.clone(),
        token: "pcxd_wrong".into(),
    };
    let (dropped, _) = mpsc::unbounded_channel();
    tokio::time::timeout(
        Duration::from_secs(5),
        daemon::run(
            connection,
            move |_: &Welcome| router(dropped.clone()),
            hello,
            CancellationToken::new(),
        ),
    )
    .await
    .expect("the daemon stops trying");
    assert_eq!(pair.admitted.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_closed_tunnel_is_opened_again_by_the_daemon() {
    let mut pair = start(Limits::default(), TOKEN).await;
    let first = pair.link().await;
    first.close();
    tokio::time::timeout(Duration::from_secs(5), first.closed())
        .await
        .expect("closed");
    let refused = first.send(request("GET", "/big", Bytes::new())).await;
    assert!(matches!(refused, Err(TunnelError::Closed)), "{refused:?}");
    tokio::time::timeout(Duration::from_secs(5), async {
        while pair.admitted.load(Ordering::SeqCst) < 2 {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the daemon connected again");
    let again = pair.link().await;
    let echo = again
        .send(request("POST", "/echo", Bytes::from_static(b"back")))
        .await
        .expect("an answer");
    let got = echo
        .into_body()
        .collect()
        .await
        .expect("the body")
        .to_bytes();
    assert_eq!(got, Bytes::from_static(b"back"));
}
