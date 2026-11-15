// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Ops as the daemon composes it (ARCHITECTURE section 8, operations, W31),
//! with the real board, team and sandbox and a fake Kubernetes API on
//! localhost: an action the team's trust names runs at once, approved by
//! the policy; one it does not name waits on the board until a person
//! approves it, then runs. The account's token is read from a file through
//! the sandbox and sent as a bearer.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use pcx_board::BoardModule;
use pcx_host::{Host, Origin};
use pcx_module_api::sandbox::{Policy, Root};
use pcx_module_api::{Module, Scope};
use pcx_ops::OpsModule;
use pcx_sandbox::{Launcher, SandboxModule};
use pcx_team::TeamModule;
use post_cortex_memory::capabilities::CreateWorkspace;
use post_cortex_proto::board::DecisionRequest;
use post_cortex_proto::board::boards_client::BoardsClient;
use post_cortex_proto::team::teams_client::TeamsClient;
use post_cortex_proto::team::{
    AddAgentRequest, Agent, Authority, Class, CreateTeamRequest, Mode, OperationsLimit,
    SetAutonomyRequest, Trust,
};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use tonic::Request;

/// What the fake cluster was sent: method, path, media type, body.
type Sent = Arc<parking_lot::Mutex<Vec<(String, String, String, String)>>>;

async fn kube(
    State(sent): State<Sent>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: String,
) -> (StatusCode, String) {
    if headers.get("authorization").and_then(|v| v.to_str().ok()) != Some("Bearer t0ken") {
        return (StatusCode::UNAUTHORIZED, "{}".into());
    }
    let media = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    sent.lock()
        .push((method.to_string(), uri.path().to_string(), media, body));
    (StatusCode::OK, json!({"spec": {"replicas": 2}}).to_string())
}

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

/// `message` as a request of person `author` (the app).
fn as_person<T>(author: &str, message: T) -> Request<T> {
    let mut request = Request::new(message);
    scope(author).to_metadata(request.metadata_mut()).unwrap();
    request
}

#[tokio::test]
async fn a_trusted_action_runs_at_once_and_another_waits_for_a_person() {
    // The daemon reads the system's proxy settings and TLS roots as it
    // starts (`daemon::proxies::warm`); so does this composition, or its
    // first HTTP client pays them within the first ops call's deadline
    // (seconds on macOS under load).
    tokio::task::spawn_blocking(|| reqwest::Client::builder().build().unwrap())
        .await
        .unwrap();
    let sent = Sent::default();
    let app = Router::new().fallback(kube).with_state(Arc::clone(&sent));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await });

    let (data, creds, scratch) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    let token = creds.path().join("prod.token");
    std::fs::write(&token, "t0ken\n").unwrap();
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
    let modules: Vec<Arc<dyn Module>> = vec![
        crate::memory_tests::memory_module(&memory).0,
        Arc::new(BoardModule::new()),
        Arc::new(TeamModule::new()),
        Arc::new(OpsModule::new()),
        Arc::new(SandboxModule::new(
            Launcher::platform(None).unwrap(),
            scratch.path(),
        )),
    ];
    let config = HashMap::from([(
        OpsModule::ID.to_string(),
        json!({"workspace_id": ws, "notify": "",
               "environments": [{"name": "prod", "kube_api": api,
                                 "kube_token_file": token.to_str().unwrap()}]}),
    )]);
    let policies = HashMap::from([(
        OpsModule::ID.to_string(),
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
    let host = Arc::new(Host::start(modules, &config, &policies).await.unwrap());
    let stop = CancellationToken::new();
    let feed = tokio::spawn(crate::compose::feed_memory_writes(
        Arc::clone(&host),
        Arc::clone(&memory),
        stop.clone(),
    ));
    let call = |author: &'static str, tool: &'static str, input: Value| {
        let host = Arc::clone(&host);
        async move {
            host.call_for_agent(&scope(author), tool, input)
                .await
                .unwrap()
                .data
                .unwrap()
        }
    };

    let client = host.channel(Origin::Client);
    let mut teams = TeamsClient::new(client.clone());
    for (name, authority) in [
        ("julius", Authority::Close),
        ("eq-medic", Authority::Decide),
    ] {
        teams
            .clone()
            .add_agent(as_person(
                "julius",
                AddAgentRequest {
                    workspace_id: ws.clone(),
                    agent: Some(Agent {
                        name: name.into(),
                        role: "ops".into(),
                        authority: authority.into(),
                        ..Agent::default()
                    }),
                },
            ))
            .await
            .unwrap();
    }
    teams
        .clone()
        .create_team(as_person(
            "julius",
            CreateTeamRequest {
                workspace_id: ws.clone(),
                name: "ops".into(),
                members: vec!["julius".into(), "eq-medic".into()],
            },
        ))
        .await
        .unwrap();
    teams
        .set_autonomy(as_person(
            "julius",
            SetAutonomyRequest {
                workspace_id: ws.clone(),
                team: "ops".into(),
                version: 0,
                trust: Some(Trust {
                    mode: Mode::Classes.into(),
                    classes: vec![Class::Operations.into()],
                    operations: Some(OperationsLimit {
                        actions: vec!["restart".into()],
                        environments: vec!["prod".into()],
                    }),
                    ..Trust::default()
                }),
                exceptions: None,
            },
        ))
        .await
        .unwrap();
    let board = call(
        "julius",
        "board_create",
        json!({"workspace_id": ws, "name": "eq-ops",
        "columns": ["Triage", "Done"]}),
    )
    .await;
    let task = call(
        "julius",
        "task_add",
        json!({"workspace_id": ws, "board": board["id"],
        "title": "svc-music crash-loops"}),
    )
    .await;
    let ask = json!({"environment": "prod", "workspace_id": ws, "task": task["id"],
        "why": "7 restarts in 10 minutes, CrashLoopBackOff"});

    let restarted = call(
        "eq-medic",
        "ops_restart",
        json!({"ask": ask,
        "namespace": "equal-rust", "name": "svc-music"}),
    )
    .await;
    assert_eq!(restarted["action"]["state"], "STATE_DONE", "{restarted}");
    let patch = sent
        .lock()
        .iter()
        .find(|(m, ..)| m == "PATCH")
        .cloned()
        .unwrap();
    assert_eq!(
        patch.1,
        "/apis/apps/v1/namespaces/equal-rust/deployments/svc-music"
    );
    assert_eq!(patch.2, "application/merge-patch+json");
    assert!(patch.3.contains("kubectl.kubernetes.io/restartedAt"));

    // Another deployment: the same target is held for 30 minutes.
    let scaled = call(
        "eq-medic",
        "ops_scale",
        json!({"ask": ask,
        "namespace": "equal-rust", "name": "svc-feed", "replicas": 3}),
    )
    .await;
    assert_eq!(
        scaled["action"]["state"], "STATE_WAITING",
        "scale is not in the trust"
    );
    let patches = || sent.lock().iter().filter(|(m, ..)| m == "PATCH").count();
    assert_eq!(patches(), 1, "nothing ran before approval");

    let thread = call(
        "julius",
        "task_get",
        json!({"workspace_id": ws, "task": task["id"]}),
    )
    .await;
    let decisions = thread["decisions"].as_array().unwrap();
    assert!(
        decisions[0]["decision"]["approved_by"]
            .as_str()
            .unwrap()
            .starts_with("policy:")
    );
    assert_eq!(decisions[1]["pending"], json!(true));
    BoardsClient::new(client)
        .approve_decision(as_person(
            "julius",
            DecisionRequest {
                workspace_id: ws.clone(),
                decision: decisions[1]["id"].as_str().unwrap().to_string(),
                version: decisions[1]["version"].as_str().unwrap().parse().unwrap(),
                note: None,
            },
        ))
        .await
        .unwrap();
    for _ in 0..100 {
        if patches() == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let scale = sent
        .lock()
        .iter()
        .filter(|(m, ..)| m == "PATCH")
        .nth(1)
        .cloned()
        .unwrap();
    assert_eq!(
        scale.1,
        "/apis/apps/v1/namespaces/equal-rust/deployments/svc-feed/scale"
    );
    assert_eq!(scale.3, r#"{"spec":{"replicas":3}}"#);
    stop.cancel();
    feed.await.unwrap();
}
