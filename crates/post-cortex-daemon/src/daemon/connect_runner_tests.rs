// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A daemon as a runner (docs/connect.md section 7): its `Hello` offers the
//! tool modules it lends with their manifests; a runner's exchange (the
//! gateway's `x-pcx-use: runner`) reaches their services only, under the
//! modules' sandbox, its denied names included, and never the daemon's
//! memory. A space's exchange, or any client's call held to a reach,
//! reaches no tool module (docs/connect.md section 4). A hosted tenant's
//! host runs a lent module as its own (`pcx_host::RemoteModule`).

use std::sync::Arc;

use axum::body::Body;
use axum::http::Request;
use pcx_host::{Grant, RootGrant};
use pcx_tunnel::gateway::Link;
use post_cortex_proto::tools_fs::{ReadRequest, ReadResponse};
use post_cortex_proto::tunnel::Offer;
use prost::Message as _;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::compose::{Builtin, BuiltinSpec, Modules};
use crate::daemon::connect_tests::{connected_with, memory};

/// `path` called over `link` with `message`, as a runner's call when
/// `runner`: its gRPC status and answer bytes.
async fn call(
    link: &Link,
    path: &str,
    message: &impl prost::Message,
    runner: bool,
) -> (String, bytes::Bytes) {
    let encoded = message.encode_to_vec();
    let mut framed = vec![0u8];
    framed.extend_from_slice(&u32::try_from(encoded.len()).unwrap().to_be_bytes());
    framed.extend_from_slice(&encoded);
    let mut request = Request::builder()
        .method("POST")
        .uri(path)
        .header("host", "daemon.internal")
        .header("content-type", "application/grpc")
        .header("te", "trailers");
    if runner {
        request = request.header(pcx_tunnel::USE_HEADER, pcx_tunnel::USE_RUNNER);
    }
    let answer = link
        .send(request.body(Body::from(framed)).unwrap())
        .await
        .unwrap();
    if let Some(status) = answer.headers().get("grpc-status") {
        return (status.to_str().unwrap().to_string(), bytes::Bytes::new());
    }
    let collected = http_body_util::BodyExt::collect(answer.into_body())
        .await
        .unwrap();
    let status = collected.trailers().map_or_else(
        || "0".to_string(),
        |t| t["grpc-status"].to_str().unwrap().to_string(),
    );
    (status, collected.to_bytes())
}

/// The modules of a daemon over `memory` running `tools-fs` over `repo`,
/// `.env` denied.
async fn with_files(
    memory: &Arc<post_cortex_memory::Memory>,
    repo: &tempfile::TempDir,
    scratch: &tempfile::TempDir,
) -> Modules {
    Modules::memory(Arc::clone(memory))
        .unwrap()
        .sandboxed(scratch.path(), vec![".env".into()], None)
        .unwrap()
        .configured(
            &[],
            &[],
            &[BuiltinSpec {
                id: Builtin::ToolsFs,
                grants: vec![Grant::Filesystem],
                filesystem: vec![RootGrant {
                    path: repo.path().to_path_buf(),
                    write: false,
                }],
                config: None,
            }],
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn a_runner_lends_its_tool_modules_and_nothing_else() {
    let (data, repo, scratch, config) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    std::fs::write(repo.path().join("main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(repo.path().join(".env"), "KEY=secret\n").unwrap();
    let memory = memory(&data).await;
    let modules = with_files(&memory, &repo, &scratch).await;
    let mut background = JoinSet::new();
    let ct = CancellationToken::new();
    let (hello, link) = connected_with(
        &memory,
        &modules,
        &[Builtin::ToolsFs],
        Vec::new(),
        &config,
        &mut background,
        &ct,
    )
    .await;
    assert!(hello.offers.contains(&(Offer::Runner as i32)));
    assert_eq!(hello.lent.len(), 1);
    assert_eq!(hello.lent[0].id, "tools-fs");

    let read = |path: &str| ReadRequest {
        path: path.into(),
        ..ReadRequest::default()
    };
    let files = "/pcx.tools.fs.v1.Files/Read";
    let (status, bytes) = call(&link, files, &read("main.rs"), true).await;
    assert_eq!(status, "0");
    let answer = ReadResponse::decode(&bytes[5..]).unwrap();
    assert_eq!(answer.lines, vec!["fn main() {}".to_string()]);

    // A denied name, through the runner as anywhere.
    let (status, _) = call(&link, files, &read(".env"), true).await;
    assert_eq!(status, "7", "PermissionDenied");
    // The daemon's memory is no service a runner lends.
    let (status, _) = call(
        &link,
        "/pcx.v1.PostCortex/ListWorkspaces",
        &post_cortex_proto::pb::ListWorkspacesRequest::default(),
        true,
    )
    .await;
    assert_eq!(status, "7", "PermissionDenied");
    // A person of the space runs nothing on the machine.
    let (status, _) = call(&link, files, &read("main.rs"), false).await;
    assert_eq!(status, "7", "PermissionDenied");

    ct.cancel();
    background.join_all().await;
}

/// An agent's call over MCP: from this machine it reads; held to a reach
/// (from the cloud), refused.
#[tokio::test]
async fn a_tool_module_refuses_a_call_from_the_cloud() {
    let (data, repo, scratch) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    std::fs::write(repo.path().join("main.rs"), "fn main() {}\n").unwrap();
    let memory = memory(&data).await;
    let host = crate::compose::host(&with_files(&memory, &repo, &scratch).await)
        .await
        .unwrap();
    let scope = |reach: Option<Vec<String>>| pcx_module_api::Scope {
        tenant: String::new(),
        profile: String::new(),
        session_id: None,
        workspace_id: None,
        author: None,
        caller_module: None,
        call_depth: 0,
        turn: None,
        reach,
        role: None,
        read_only: false,
        person: None,
        deadline: std::time::Instant::now() + std::time::Duration::from_secs(10),
        cancel: CancellationToken::new(),
    };
    let input = serde_json::json!({ "path": "main.rs" });
    host.call_for_agent(&scope(None), "read", input.clone())
        .await
        .unwrap();
    let refused = host
        .call_for_agent(&scope(Some(Vec::new())), "read", input)
        .await
        .unwrap_err();
    assert_eq!(refused.kind, pcx_module_api::ModuleErrorKind::Denied);
    assert!(
        refused.message.contains("not a call from the cloud"),
        "{refused:?}"
    );
    host.stop().await;
}

/// A tenant's host runs the module the runner lends, from the manifest of
/// its `Hello`, its calls forwarded over the tunnel: an agent's tool call
/// there reads the runner's file.
#[tokio::test]
async fn a_tenants_host_runs_a_lent_module_as_its_own() {
    let (data, tenant_data, repo, scratch, config) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    std::fs::write(repo.path().join("main.rs"), "fn main() {}\n").unwrap();
    let tenant_memory = memory(&tenant_data).await;
    let memory = memory(&data).await;
    let modules = with_files(&memory, &repo, &scratch).await;
    let mut background = JoinSet::new();
    let ct = CancellationToken::new();
    let (hello, link) = connected_with(
        &memory,
        &modules,
        &[Builtin::ToolsFs],
        Vec::new(),
        &config,
        &mut background,
        &ct,
    )
    .await;
    // As the gateway forwards a runner's call.
    let to_runner = pcx_module_api::grpc::channel(tower::service_fn(
        move |mut request: Request<tonic::body::Body>| {
            let link = link.clone();
            async move {
                request.headers_mut().insert(
                    pcx_tunnel::USE_HEADER,
                    axum::http::HeaderValue::from_static(pcx_tunnel::USE_RUNNER),
                );
                link.send(request)
                    .await
                    .map(|answer| answer.map(tonic::body::Body::new))
            }
        },
    ));
    let tenant = Modules::memory(tenant_memory)
        .unwrap()
        .with_remote(vec![Arc::new(pcx_host::RemoteModule::new(
            hello.lent[0].clone(),
            to_runner,
        ))]);
    let host = crate::compose::host(&tenant).await.unwrap();
    let scope = pcx_module_api::Scope {
        tenant: String::new(),
        profile: String::new(),
        session_id: None,
        workspace_id: None,
        author: Some("dev".into()),
        caller_module: None,
        call_depth: 0,
        turn: None,
        reach: None,
        role: None,
        read_only: false,
        person: None,
        deadline: std::time::Instant::now() + std::time::Duration::from_secs(10),
        cancel: CancellationToken::new(),
    };
    let read = host
        .call_for_agent(&scope, "read", serde_json::json!({ "path": "main.rs" }))
        .await
        .unwrap();
    assert_eq!(
        read.data.unwrap()["lines"],
        serde_json::json!(["fn main() {}"])
    );
    host.stop().await;
    ct.cancel();
    background.join_all().await;
}
