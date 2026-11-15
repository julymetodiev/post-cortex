// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Observe through a host with the sandbox module, against a fake
//! Prometheus on localhost: its token is read from a file beneath the
//! module's filesystem grant at each call and sent as a bearer; a token
//! file outside the grant is refused, and nothing is asked.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Router;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::get;
use pcx_host::Host;
use pcx_module_api::sandbox::{Policy, Root};
use pcx_module_api::{Module, ModuleErrorKind, Scope};
use pcx_sandbox::{Launcher, SandboxModule};
use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::ObserveModule;

fn scope() -> Scope {
    Scope {
        tenant: String::new(),
        profile: String::new(),
        session_id: None,
        workspace_id: None,
        author: None,
        caller_module: None,
        call_depth: 0,
        turn: None,
        reach: None,
        role: None,
        read_only: false,
        person: None,
        deadline: Instant::now() + Duration::from_secs(10),
        cancel: CancellationToken::new(),
    }
}

/// A Prometheus that answers only the bearer `s3cret`.
async fn prometheus() -> String {
    let app = Router::new().route(
        "/api/v1/query",
        get(|headers: HeaderMap| async move {
            if headers.get("authorization").and_then(|v| v.to_str().ok()) != Some("Bearer s3cret") {
                return (StatusCode::UNAUTHORIZED, "no".to_string());
            }
            (
                StatusCode::OK,
                json!({"status": "success", "data": {"resultType": "vector",
                    "result": [{"metric": {"job": "svc-music"}, "value": [1793086200, "1"]}]}})
                .to_string(),
            )
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await });
    url
}

#[tokio::test]
async fn a_token_beneath_the_grant_is_read_at_each_call_and_sent_as_a_bearer() {
    let url = prometheus().await;
    let (creds, outside, scratch) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    let token = creds.path().join("prod.token");
    std::fs::write(&token, "s3cret\n").unwrap();
    let stray = outside.path().join("prod.token");
    std::fs::write(&stray, "s3cret\n").unwrap();
    let env = |name: &str, file: &std::path::Path| json!({"name": name, "prometheus": url, "token_file": file.to_str().unwrap()});
    let config = HashMap::from([(
        ObserveModule::ID.to_string(),
        json!({"environments": [env("prod", &token), env("stray", &stray)]}),
    )]);
    let policies = HashMap::from([(
        ObserveModule::ID.to_string(),
        Policy {
            roots: vec![Root {
                path: creds.path().to_path_buf(),
                write: false,
            }],
            network: true,
            spawn: false,
            deny: Vec::new(),
            local_only: false,
        },
    )]);
    let modules: Vec<Arc<dyn Module>> = vec![
        Arc::new(ObserveModule::new()),
        Arc::new(SandboxModule::new(
            Launcher::platform(None).unwrap(),
            scratch.path(),
        )),
    ];
    // The first HTTP client of a process reads the system's proxy settings
    // (seconds on macOS); the daemon reads them at start, and so does this
    // test, before the calls it makes under their deadline.
    reqwest::Client::builder().build().unwrap();
    let host = Host::start(modules, &config, &policies).await.unwrap();
    let query = |environment: &str| json!({"environment": environment, "query": "up"});

    let answered = host
        .call_for_agent(&scope(), "metrics_query", query("prod"))
        .await
        .unwrap()
        .data
        .unwrap();
    assert_eq!(answered["series"][0]["labels"]["job"], "svc-music");

    // A rotated token is used at once.
    std::fs::write(&token, "rotated\n").unwrap();
    let stale = host
        .call_for_agent(&scope(), "metrics_query", query("prod"))
        .await
        .unwrap_err();
    assert_eq!(stale.kind, ModuleErrorKind::FailedPrecondition, "{stale:?}");
    assert!(stale.message.contains("Prometheus refused"), "{stale:?}");

    let outside_grant = host
        .call_for_agent(&scope(), "metrics_query", query("stray"))
        .await
        .unwrap_err();
    assert_eq!(
        outside_grant.kind,
        ModuleErrorKind::Denied,
        "{outside_grant:?}"
    );
}
