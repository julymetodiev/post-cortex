// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The routes of a memory instance (`compose::routes`): no browser routes
//! unless asked; with origins, CORS for those only, gRPC-Web and the
//! browser transfer behind the token, and a single-use download link that
//! needs none.

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use post_cortex_memory::Memory;
use post_cortex_storage::RealRocksDBStorage;
use post_cortex_transport::browser::Origins;
use prost::Message;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

use crate::compose::{BrowserAccess, RouteConfig, routes};

const TOKEN: &str = "s3cret";
const ORIGIN: &str = "https://app.example.com";

async fn memory(dir: &tempfile::TempDir) -> Arc<Memory> {
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

async fn app(memory: Arc<Memory>, browser: BrowserAccess) -> Router {
    let modules = crate::compose::Modules::memory(Arc::clone(&memory)).unwrap();
    let host = Arc::new(crate::compose::host(&modules).await.unwrap());
    routes(
        host,
        memory,
        modules.kinds(),
        None,
        RouteConfig {
            host_policy: crate::daemon::host_allowlist::HostPolicy::from_config(
                &crate::daemon::DaemonConfig {
                    host: "127.0.0.1".to_string(),
                    ..Default::default()
                },
            )
            .unwrap(),
            auth_token: Some(TOKEN.into()),
            browser,
        },
        &CancellationToken::new(),
    )
}

fn request(method: &str, uri: &str, token: bool, body: impl Into<Body>) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "127.0.0.1")
        .header("origin", ORIGIN);
    if token {
        builder = builder.header("authorization", format!("Bearer {TOKEN}"));
    }
    builder.body(body.into()).unwrap()
}

async fn body(response: axum::response::Response) -> Vec<u8> {
    axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec()
}

#[tokio::test]
async fn a_daemon_serves_no_browser_routes_unless_configured() {
    let dir = tempfile::tempdir().unwrap();
    let app = app(memory(&dir).await, BrowserAccess::Off).await;
    let response = app
        .oneshot(request("POST", "/v1/export", true, ""))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn browsers_export_import_and_call_grpc_web() {
    let dir = tempfile::tempdir().unwrap();
    let memory = memory(&dir).await;
    memory
        .sessions
        .create(Some("browser".into()), None)
        .await
        .unwrap();
    let origins = Origins::parse(&[ORIGIN.to_string()]).unwrap();
    let app = app(memory, BrowserAccess::Origins(origins)).await;

    // Preflight from the allowed origin.
    let preflight = Request::builder()
        .method("OPTIONS")
        .uri("/v1/import")
        .header("host", "127.0.0.1")
        .header("origin", ORIGIN)
        .header("access-control-request-method", "POST")
        .body(Body::empty())
        .unwrap();
    let answered = app.clone().oneshot(preflight).await.unwrap();
    assert_eq!(answered.headers()["access-control-allow-origin"], ORIGIN);

    // A link needs the token; the download does not, and works once.
    let refused = app
        .clone()
        .oneshot(request("POST", "/v1/export", false, ""))
        .await
        .unwrap();
    assert_eq!(refused.status(), StatusCode::UNAUTHORIZED);
    let linked = app
        .clone()
        .oneshot(request(
            "POST",
            "/v1/export",
            true,
            r#"{"compression":"gzip"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(linked.status(), StatusCode::OK);
    let link: serde_json::Value = serde_json::from_slice(&body(linked).await).unwrap();
    let url = link["url"].as_str().unwrap().to_string();
    assert!(link["filename"].as_str().unwrap().ends_with(".json.gz"));
    let downloaded = app
        .clone()
        .oneshot(request("GET", &url, false, ""))
        .await
        .unwrap();
    assert_eq!(downloaded.status(), StatusCode::OK);
    assert!(
        downloaded.headers()["content-disposition"]
            .to_str()
            .unwrap()
            .starts_with("attachment")
    );
    let archive = body(downloaded).await;
    assert!(archive.starts_with(&[0x1f, 0x8b]), "a gzip archive");
    let again = app
        .clone()
        .oneshot(request("GET", &url, false, ""))
        .await
        .unwrap();
    assert_eq!(again.status(), StatusCode::NOT_FOUND);

    // The archive previews against the instance it came from.
    let previewed = app
        .clone()
        .oneshot(request("POST", "/v1/import?commit=false", true, archive))
        .await
        .unwrap();
    assert_eq!(previewed.status(), StatusCode::OK);
    let preview: serde_json::Value = serde_json::from_slice(&body(previewed).await).unwrap();
    assert_eq!(preview["committed"], false);
    assert_eq!(preview["sessions"], 1);
    assert_eq!(preview["conflicts"][0]["kind"], "session");
    let bad = app
        .clone()
        .oneshot(request("POST", "/v1/import?commit=perhaps", true, "x"))
        .await
        .unwrap();
    assert_eq!(bad.status(), StatusCode::BAD_REQUEST);

    // gRPC-Web, behind the token.
    let message = post_cortex_proto::views::ListConversationsRequest {
        scope: Some(post_cortex_proto::views::Scope {
            scope: Some(post_cortex_proto::views::scope::Scope::Global(true)),
        }),
        ..Default::default()
    }
    .encode_to_vec();
    let mut frame = vec![0];
    frame.extend_from_slice(&(message.len() as u32).to_be_bytes());
    frame.extend_from_slice(&message);
    let call = |token: bool| {
        let mut r = request(
            "POST",
            "/pcx.views.v1.Views/ListConversations",
            token,
            frame.clone(),
        );
        r.headers_mut().insert(
            "content-type",
            "application/grpc-web+proto".parse().unwrap(),
        );
        r
    };
    let answered = app.clone().oneshot(call(true)).await.unwrap();
    assert_eq!(answered.status(), StatusCode::OK);
    let text = String::from_utf8_lossy(&body(answered).await).to_string();
    assert!(text.contains("grpc-status:0"), "{text}");
    let unauthenticated = app.oneshot(call(false)).await.unwrap();
    let status = unauthenticated
        .headers()
        .get("grpc-status")
        .map(|v| v.to_str().unwrap().to_string());
    let text = String::from_utf8_lossy(&body(unauthenticated).await).to_string();
    assert!(
        status.as_deref() == Some("16") || text.contains("grpc-status:16"),
        "{status:?} {text}"
    );
}

#[tokio::test]
async fn behind_a_gateway_links_point_where_it_routes() {
    let dir = tempfile::tempdir().unwrap();
    let browser = BrowserAccess::Proxied {
        link_base: "/v1/orgs/acme".into(),
    };
    let app = app(memory(&dir).await, browser).await;
    let linked = app
        .clone()
        .oneshot(request("POST", "/v1/export", true, ""))
        .await
        .unwrap();
    assert_eq!(linked.status(), StatusCode::OK);
    let link: serde_json::Value = serde_json::from_slice(&body(linked).await).unwrap();
    let url = link["url"].as_str().unwrap();
    let ticket = url.strip_prefix("/v1/orgs/acme/export/").unwrap();
    // The gateway strips its prefix: the instance serves the plain path.
    let downloaded = app
        .oneshot(request("GET", &format!("/v1/export/{ticket}"), false, ""))
        .await
        .unwrap();
    assert_eq!(downloaded.status(), StatusCode::OK);
}
