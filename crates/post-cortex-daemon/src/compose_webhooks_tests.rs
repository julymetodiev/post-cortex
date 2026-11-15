// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The webhooks as the daemon composes them (ARCHITECTURE section 8,
//! operations, W27): Alertmanager's request on `POST /v1/webhooks/{source}`
//! becomes one task on the real board, commented when it resolves, and the
//! incident recorded in the board's session; a request without the token
//! changes nothing.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use pcx_board::BoardModule;
use pcx_host::{Host, Origin};
use pcx_module_api::{Module, Scope};
use pcx_webhooks::WebhooksModule;
use post_cortex_memory::capabilities::CreateWorkspace;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

const FIRING: &str = r#"{"version": "4", "status": "firing", "receiver": "pcx",
  "groupKey": "{}:{alertname=\"DiskFull\"}", "truncatedAlerts": 0,
  "groupLabels": {"alertname": "DiskFull", "instance": "db-1"},
  "commonLabels": {"alertname": "DiskFull", "instance": "db-1", "severity": "critical"},
  "commonAnnotations": {"summary": "Disk 95% full"}, "externalURL": "https://am",
  "alerts": [{"status": "firing", "labels": {"alertname": "DiskFull", "instance": "db-1"},
    "annotations": {}, "startsAt": "2026-10-27T07:00:00Z", "endsAt": "0001-01-01T00:00:00Z",
    "generatorURL": "https://prom", "fingerprint": "f1"}]}"#;

fn scope(author: &str) -> Scope {
    Scope {
        tenant: String::new(),
        profile: String::new(),
        session_id: None,
        workspace_id: None,
        author: Some(author.to_string()),
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

async fn call(host: &Host, tool: &str, input: Value) -> Value {
    host.call_for_agent(&scope("julius"), tool, input)
        .await
        .unwrap()
        .data
        .unwrap()
}

async fn post(host: &Host, source: &str, token: &str, body: &str) -> StatusCode {
    let routes = post_cortex_transport::webhooks::routes(host.channel(Origin::Client));
    let request = Request::post(format!("/v1/webhooks/{source}"))
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    routes.oneshot(request).await.unwrap().status()
}

#[tokio::test]
async fn an_alert_on_the_route_is_one_task_on_the_board_until_it_resolves() {
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
    let ws = memory
        .workspaces
        .create(CreateWorkspace {
            name: "Equal".into(),
            description: String::new(),
        })
        .await
        .unwrap()
        .id
        .to_string();
    let modules = || -> Vec<Arc<dyn Module>> {
        vec![
            crate::memory_tests::memory_module(&memory).0,
            Arc::new(BoardModule::new()),
            Arc::new(WebhooksModule::new()),
        ]
    };
    // The board first: the webhooks' source names it.
    let host = Host::start(modules()[..2].to_vec(), &HashMap::new(), &HashMap::new())
        .await
        .unwrap();
    let board = call(
        &host,
        "board_create",
        json!({"workspace_id": ws, "name": "eq-ops", "columns": ["Triage", "Done"]}),
    )
    .await;
    let board_id = board["id"].as_str().unwrap().to_string();
    let session = board["board"]["session_id"].as_str().unwrap().to_string();
    let config = HashMap::from([(
        WebhooksModule::ID.to_string(),
        json!({"token": "s3cret", "sources": [{
            "name": "alertmanager", "kind": "SOURCE_KIND_ALERTMANAGER",
            "workspace_id": ws, "board": board_id, "assignee": "eq-sentinel"}]}),
    )]);
    host.reload(modules(), &config, &HashMap::new())
        .await
        .unwrap();

    assert_eq!(
        post(&host, "alertmanager", "guess", FIRING).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        post(&host, "github", "s3cret", FIRING).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        post(&host, "alertmanager", "s3cret", "{oops").await,
        StatusCode::BAD_REQUEST
    );
    let tasks = || {
        call(
            &host,
            "task_list",
            json!({"workspace_id": ws, "board": board_id}),
        )
    };
    assert_eq!(
        tasks().await["tasks"].as_array().map(Vec::len),
        Some(0),
        "nothing refused got in"
    );

    assert_eq!(
        post(&host, "alertmanager", "s3cret", FIRING).await,
        StatusCode::OK
    );
    assert_eq!(
        post(&host, "alertmanager", "s3cret", FIRING).await,
        StatusCode::OK
    );
    let listed = tasks().await;
    let added = &listed["tasks"].as_array().unwrap()[..];
    assert_eq!(added.len(), 1, "one task for the group");
    let task = &added[0];
    assert_eq!(task["task"]["title"], "DiskFull · db-1");
    assert_eq!(task["task"]["assignee"], "eq-sentinel");
    assert_eq!(task["task"]["priority"], "P0");
    assert_eq!(task["author"], "alertmanager", "the source added it");

    let resolved = FIRING
        .replace(r#""status": "firing""#, r#""status": "resolved""#)
        .replace("0001-01-01T00:00:00Z", "2026-10-27T07:12:00Z");
    assert_eq!(
        post(&host, "alertmanager", "s3cret", &resolved).await,
        StatusCode::OK
    );
    let id = task["id"].as_str().unwrap();
    let thread = call(&host, "task_get", json!({"workspace_id": ws, "task": id})).await;
    let said: Vec<&str> = thread["comments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["text"].as_str().unwrap())
        .collect();
    assert_eq!(
        said,
        ["Fires again: 1 of 1 alerts.", "Resolved after 12 min."]
    );
    let recorded = memory.sessions.get(session.parse().unwrap()).await.unwrap();
    assert!(
        recorded
            .records()
            .iter()
            .any(|r| r.content.title.contains("Incident: DiskFull resolved")),
        "the incident is in the board's session"
    );
}
