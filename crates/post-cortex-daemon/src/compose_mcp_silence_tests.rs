// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A long tool call over MCP keeps its connection: while the tool runs, the
//! answer's stream carries keep-alives, which is what a proxy with a limit on
//! silence (Cloudflare's 120 s) needs.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{Request, header};
use futures::StreamExt;
use pcx_agent::AgentModule;
use pcx_board::BoardModule;
use pcx_chief::ChiefModule;
use pcx_host::Host;
use pcx_module_api::Module;
use pcx_team::TeamModule;
use post_cortex_memory::capabilities::CreateWorkspace;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

use crate::compose_scripted_tests::{Provider, Scripted};
use crate::daemon::host_allowlist::HostPolicy;
use crate::daemon::rmcp_server::build_router;

/// A request of the sessions' protocol; `session`: that session's.
fn request(body: &Value, session: Option<&str>) -> Request<Body> {
    let mut req = Request::post("/mcp")
        .header(header::HOST, "localhost:3737")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ACCEPT, "application/json, text/event-stream")
        .header("x-pcx-author", "julius");
    if let Some(session) = session {
        req = req.header("mcp-session-id", session);
    }
    req.body(Body::from(body.to_string())).unwrap()
}

/// What the test keeps of a running host.
struct Running {
    router: axum::Router,
    host: Arc<Host>,
    ct: CancellationToken,
    _data: tempfile::TempDir,
}

/// A host whose chief's model never answers: `chief_ask` runs on.
async fn running() -> Running {
    let data = tempfile::tempdir().unwrap();
    let memory = Arc::new(
        crate::memory_tests::rocksdb_memory(
            &data,
            post_cortex_memory::SystemConfig {
                enable_embeddings: false,
                ..Default::default()
            },
        )
        .await
        .unwrap(),
    );
    let own = memory
        .workspaces
        .create(CreateWorkspace {
            name: "chief".into(),
            description: String::new(),
        })
        .await
        .unwrap()
        .id
        .to_string();
    // Its one answer never comes: the tool runs on.
    let scripted = Scripted::with(vec![Vec::new()]);
    let modules: Vec<Arc<dyn Module>> = vec![
        crate::memory_tests::memory_module(&memory).0,
        Arc::new(Provider(Arc::clone(&scripted))),
        Arc::new(BoardModule::new()),
        Arc::new(TeamModule::new()),
        Arc::new(AgentModule::new()),
        Arc::new(ChiefModule::new()),
    ];
    let config = HashMap::from([(ChiefModule::ID.to_string(), json!({ "workspace_id": own }))]);
    let host = Arc::new(
        Host::start(modules, &config, &HashMap::new())
            .await
            .unwrap(),
    );
    let ct = CancellationToken::new();
    let router = build_router(
        Arc::clone(&host),
        Arc::clone(&memory),
        &HostPolicy::Only(vec!["localhost".into()]),
        None,
        &ct,
    );
    Running {
        router,
        host,
        ct,
        _data: data,
    }
}

/// `answer`'s frames until `until` from `started`: at least two after the
/// tool has been running for `quiet`.
async fn keeps_sending(
    answer: axum::response::Response,
    started: Instant,
    quiet: Duration,
    until: Duration,
) {
    let mut frames = answer.into_body().into_data_stream();
    let mut seen = Vec::new();
    while started.elapsed() < until {
        let next = tokio::time::timeout(Duration::from_secs(5), frames.next()).await;
        if let Ok(Some(frame)) = next {
            seen.push((started.elapsed(), frame.unwrap()));
        }
    }
    assert!(
        seen.iter().filter(|(at, _)| *at > quiet).count() >= 2,
        "silent while the tool runs: {seen:?}"
    );
}

/// The sessions' protocol: the answer's head comes at once, then keep-alives.
#[tokio::test(flavor = "multi_thread")]
async fn a_long_tool_call_sends_keep_alives_while_it_runs() {
    let Running {
        router,
        host,
        ct,
        _data,
    } = running().await;
    let init = json!({
        "jsonrpc": "2.0", "id": 0, "method": "initialize",
        "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                   "clientInfo": {"name": "test", "version": "0"}},
    });
    let answer = router.clone().oneshot(request(&init, None)).await.unwrap();
    let session = answer.headers()["mcp-session-id"]
        .to_str()
        .unwrap()
        .to_string();
    let initialized = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
    router
        .clone()
        .oneshot(request(&initialized, Some(&session)))
        .await
        .unwrap();

    let started = Instant::now();
    let call = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": "chief_ask", "arguments": {"question": "Hold on"}},
    });
    let answer = tokio::time::timeout(
        Duration::from_secs(5),
        router.clone().oneshot(request(&call, Some(&session))),
    )
    .await
    .expect("its head waits for the tool")
    .unwrap();
    keeps_sending(
        answer,
        started,
        Duration::from_secs(10),
        Duration::from_secs(40),
    )
    .await;
    ct.cancel();
    host.stop().await;
}

/// The protocol without sessions (2026-07-28, what Claude Code speaks): each
/// request names its version. A proxy with a limit on silence must see the
/// answer's head long before that limit, while the tool still runs.
#[tokio::test(flavor = "multi_thread")]
async fn a_long_tool_call_without_a_session_answers_its_head_while_it_runs() {
    let Running {
        router,
        host,
        ct,
        _data,
    } = running().await;
    let call = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {
            "name": "chief_ask",
            "arguments": {"question": "Hold on"},
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientCapabilities": {},
                "io.modelcontextprotocol/clientInfo": {"name": "test", "version": "0"},
            },
        },
    });
    let mut asked = request(&call, None);
    let headers = asked.headers_mut();
    headers.insert("mcp-protocol-version", "2026-07-28".parse().unwrap());
    headers.insert("mcp-method", "tools/call".parse().unwrap());
    headers.insert("mcp-name", "chief_ask".parse().unwrap());

    let started = Instant::now();
    let answer = tokio::time::timeout(Duration::from_secs(30), router.clone().oneshot(asked))
        .await
        .expect("its head waits for the tool")
        .unwrap();
    assert_eq!(answer.status(), 200);
    keeps_sending(
        answer,
        started,
        Duration::from_secs(10),
        Duration::from_secs(50),
    )
    .await;
    ct.cancel();
    host.stop().await;
}
