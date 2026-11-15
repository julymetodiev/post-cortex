// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A paired daemon serves its routes over the tunnel: a gateway on a
//! loopback port receives its `Hello` with its workspaces and reaches its
//! REST routes through the link, with no bearer of the daemon.

use std::sync::Arc;
use std::time::Duration;

use prost::Message as _;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use pcx_tunnel::gateway::{self, Limits, Link};
use pcx_tunnel::pairing::Paired;
use post_cortex_memory::capabilities::CreateWorkspace;
use post_cortex_proto::tunnel::tunnel_server::{Tunnel, TunnelServer};
use post_cortex_proto::tunnel::{DaemonFrame, Hello, LeaveRequest, Left, Welcome};
use post_cortex_storage::RealRocksDBStorage;
use tokio::sync::watch;
use tokio::task::JoinSet;
use tokio_stream::wrappers::TcpListenerStream;
use tokio_util::sync::CancellationToken;

const TOKEN: &str = "pcxd_paired";

/// A gateway keeping the daemon's hello and link, its space offering
/// `offered`.
struct Gateway {
    seen: watch::Sender<Option<(Hello, Link)>>,
    offered: Vec<String>,
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
        let seen = self.seen.clone();
        let offered = self.offered.clone();
        let frames = gateway::serve(
            request.into_inner(),
            Limits::default(),
            move |hello, link| async move {
                seen.send_replace(Some((hello, link)));
                Ok(Welcome {
                    daemon_id: "d1".into(),
                    workspaces: offered,
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

pub async fn memory(dir: &tempfile::TempDir) -> Arc<post_cortex_memory::Memory> {
    let storage = RealRocksDBStorage::new(dir.path()).await.unwrap();
    let memory = crate::memory_tests::over_rocksdb(
        storage,
        post_cortex_memory::SystemConfig {
            enable_embeddings: false,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    Arc::new(memory)
}

/// A paired daemon over `memory` (its routes, the tunnel a task of
/// `background` until `ct` ends), connected to a test gateway whose space
/// offers `offered`: the daemon's hello and the gateway's link.
async fn connected(
    memory: &Arc<post_cortex_memory::Memory>,
    offered: Vec<String>,
    config: &tempfile::TempDir,
    background: &mut JoinSet<()>,
    ct: &CancellationToken,
) -> (Hello, Link) {
    let modules = crate::compose::Modules::memory(Arc::clone(memory)).unwrap();
    connected_with(memory, &modules, &[], offered, config, background, ct).await
}

/// As [`connected`], the daemon running `modules` and lending `lent` as a
/// runner.
pub async fn connected_with(
    memory: &Arc<post_cortex_memory::Memory>,
    modules: &crate::compose::Modules,
    lent: &[crate::compose::Builtin],
    offered: Vec<String>,
    config: &tempfile::TempDir,
    background: &mut JoinSet<()>,
    ct: &CancellationToken,
) -> (Hello, Link) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (seen_tx, mut seen) = watch::channel(None);
    let stop = ct.clone();
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(TunnelServer::new(Gateway {
                seen: seen_tx,
                offered,
            }))
            .serve_with_incoming_shutdown(TcpListenerStream::new(listener), stop.cancelled())
            .await
            .unwrap();
    });
    Paired {
        url,
        daemon_id: "d1".into(),
        org_id: "o1".into(),
        org_name: "Equal".into(),
        token: TOKEN.into(),
    }
    .save(config.path())
    .unwrap();
    let host = Arc::new(crate::compose::host(modules).await.unwrap());
    crate::daemon::connect::serve(
        config.path(),
        host,
        Arc::clone(memory),
        modules.kinds(),
        lent,
        background,
        ct,
    )
    .unwrap();
    assert_eq!(background.len(), 1, "the tunnel runs");
    tokio::time::timeout(Duration::from_secs(10), seen.wait_for(Option::is_some))
        .await
        .unwrap()
        .unwrap()
        .clone()
        .unwrap()
}

async fn workspace(memory: &post_cortex_memory::Memory, name: &str) -> String {
    memory
        .workspaces
        .create(CreateWorkspace {
            name: name.into(),
            description: String::new(),
        })
        .await
        .unwrap()
        .id
        .to_string()
}

#[tokio::test]
async fn a_paired_daemon_serves_its_routes_over_the_tunnel() {
    let data = tempfile::tempdir().unwrap();
    let memory = memory(&data).await;
    let ops = workspace(&memory, "ops").await;
    let config = tempfile::tempdir().unwrap();
    let mut background = JoinSet::new();
    let ct = CancellationToken::new();
    let (hello, link) = connected(&memory, vec![ops.clone()], &config, &mut background, &ct).await;
    assert_eq!(hello.workspaces.len(), 1);
    assert_eq!(hello.workspaces[0].id, ops);
    assert_eq!(hello.workspaces[0].name, "ops");

    let request = Request::builder()
        .uri("/api/workspaces")
        .header("host", "api.example")
        .body(Body::empty())
        .unwrap();
    let answer = link.send(request).await.unwrap();
    assert_eq!(answer.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(Body::new(answer.into_body()), usize::MAX)
        .await
        .unwrap();
    let listed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(listed[0]["name"], "ops", "{listed}");

    ct.cancel();
    background.join_all().await;
}

/// The gRPC status of `method` of `pcx.v1.PostCortex` called over `link`
/// with `message`.
async fn grpc_status(link: &Link, method: &str, message: &impl prost::Message) -> String {
    let encoded = message.encode_to_vec();
    let mut framed = vec![0u8];
    framed.extend_from_slice(&u32::try_from(encoded.len()).unwrap().to_be_bytes());
    framed.extend_from_slice(&encoded);
    let request = Request::builder()
        .method("POST")
        .uri(format!("/pcx.v1.PostCortex/{method}"))
        .header("host", "daemon.internal")
        .header("content-type", "application/grpc")
        .header("te", "trailers")
        .body(Body::from(framed))
        .unwrap();
    let answer = link.send(request).await.unwrap();
    if let Some(status) = answer.headers().get("grpc-status") {
        return status.to_str().unwrap().to_string();
    }
    let collected = http_body_util::BodyExt::collect(answer.into_body())
        .await
        .unwrap();
    let Some(trailers) = collected.trailers().cloned() else {
        panic!("{method}: no trailers; body {:?}", collected.to_bytes());
    };
    trailers["grpc-status"].to_str().unwrap().to_string()
}

/// The answer's bytes of `method` called over `link` with `message`.
async fn grpc_answer(link: &Link, method: &str, message: &impl prost::Message) -> bytes::Bytes {
    let encoded = message.encode_to_vec();
    let mut framed = vec![0u8];
    framed.extend_from_slice(&u32::try_from(encoded.len()).unwrap().to_be_bytes());
    framed.extend_from_slice(&encoded);
    let request = Request::builder()
        .method("POST")
        .uri(format!("/pcx.v1.PostCortex/{method}"))
        .header("host", "daemon.internal")
        .header("content-type", "application/grpc")
        .header("te", "trailers")
        .body(Body::from(framed))
        .unwrap();
    let answer = link.send(request).await.unwrap();
    http_body_util::BodyExt::collect(answer.into_body())
        .await
        .unwrap()
        .to_bytes()
}

#[tokio::test]
async fn a_space_reaches_only_the_workspaces_it_offers_whatever_the_call_carries() {
    let data = tempfile::tempdir().unwrap();
    let memory = memory(&data).await;
    let (ops, private) = (
        workspace(&memory, "ops").await,
        workspace(&memory, "private").await,
    );
    let config = tempfile::tempdir().unwrap();
    let mut background = JoinSet::new();
    let ct = CancellationToken::new();
    let (_, link) = connected(&memory, vec![ops.clone()], &config, &mut background, &ct).await;
    let get = |id: &str| post_cortex_proto::pb::GetWorkspaceRequest {
        workspace_id: id.to_string(),
    };
    assert_eq!(grpc_status(&link, "GetWorkspace", &get(&ops)).await, "0");
    assert_eq!(
        grpc_status(&link, "GetWorkspace", &get(&private)).await,
        "7",
        "outside the space's reach"
    );
    assert_eq!(
        grpc_status(
            &link,
            "SearchSessions",
            &post_cortex_proto::pb::SearchSessionsRequest::default()
        )
        .await,
        "7",
        "every workspace"
    );
    let listed = grpc_answer(
        &link,
        "ListWorkspaces",
        &post_cortex_proto::pb::ListWorkspacesRequest::default(),
    )
    .await;
    let listed = post_cortex_proto::pb::ListWorkspacesResponse::decode(&listed[5..]).unwrap();
    let ids: Vec<&str> = listed
        .workspaces
        .iter()
        .map(|w| w.workspace_id.as_str())
        .collect();
    assert_eq!(ids, vec![ops.as_str()], "the list is kept to the reach");

    // REST too: the list kept to the reach, a workspace outside it refused.
    let rest = |method: &str, uri: String| {
        Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "daemon.internal")
            .body(Body::empty())
            .unwrap()
    };
    let answer = link
        .send(rest("GET", "/api/workspaces".into()))
        .await
        .unwrap();
    assert_eq!(answer.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(Body::new(answer.into_body()), usize::MAX)
        .await
        .unwrap();
    let listed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 1, "{listed}");
    assert_eq!(listed[0]["id"], ops.as_str());
    let refused = link
        .send(rest("DELETE", format!("/api/workspaces/{private}")))
        .await
        .unwrap();
    assert_eq!(refused.status(), StatusCode::FORBIDDEN);
    assert!(
        memory.workspaces.get(private.parse().unwrap()).is_ok(),
        "kept"
    );
    ct.cancel();
    background.join_all().await;
}

#[tokio::test]
async fn an_unpaired_daemon_opens_no_tunnel() {
    let data = tempfile::tempdir().unwrap();
    let memory = memory(&data).await;
    let modules = crate::compose::Modules::memory(Arc::clone(&memory)).unwrap();
    let host = Arc::new(crate::compose::host(&modules).await.unwrap());
    let config = tempfile::tempdir().unwrap();
    let mut background = JoinSet::new();
    crate::daemon::connect::serve(
        config.path(),
        host,
        memory,
        modules.kinds(),
        &[],
        &mut background,
        &CancellationToken::new(),
    )
    .unwrap();
    assert!(background.is_empty());
}
