// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The reach of a call (docs/connect.md 4a) through the modules as the
//! daemon composes them: a person limited to one workspace mentions a
//! member in a comment; the member's turn, woken by that write, acts within
//! the person's reach, so a tool call of the member's model in another
//! workspace is refused.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use pcx_agent::AgentModule;
use pcx_board::BoardModule;
use pcx_chief::ChiefModule;
use pcx_host::Host;
use pcx_module_api::agent::{TurnEvent, TurnRequest, TurnsSource};
use pcx_module_api::model::Block;
use pcx_module_api::{Module, Scope, metadata};
use pcx_skills::SkillsModule;
use pcx_team::TeamModule;
use post_cortex_core::workspace::SessionRole;
use post_cortex_memory::capabilities::CreateWorkspace;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::compose_scripted_tests::{Provider, Scripted};

fn scope(author: &str, reach: Option<Vec<String>>) -> Scope {
    Scope {
        tenant: String::new(),
        profile: String::new(),
        session_id: None,
        workspace_id: None,
        author: Some(author.to_string()),
        caller_module: None,
        call_depth: 0,
        turn: None,
        reach,
        role: None,
        read_only: false,
        person: None,
        deadline: Instant::now() + Duration::from_secs(20),
        cancel: CancellationToken::new(),
    }
}

async fn call(host: &Host, scope: &Scope, tool: &str, input: Value) -> Value {
    crate::memory_tests::as_person(host, scope, tool, input)
        .await
        .unwrap()
}

/// The result of the member's tool call (its text, whether it failed),
/// in a turn woken by Ana's comment, Ana reaching `reach` (`Some(true)`:
/// the team's workspace only).
async fn woken_by_ana(limited: bool) -> (String, bool) {
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
    let workspace = |name: &'static str| {
        let memory = Arc::clone(&memory);
        async move {
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
    };
    let (ws, other) = (workspace("ops").await, workspace("private").await);
    // The member's model asks for the other workspace's documents, then
    // answers.
    let scripted = Scripted::with(vec![
        vec![Block::ToolUse {
            id: "c1".into(),
            name: "memory_search".into(),
            input: json!({ "query": "anything", "workspace_id": other }),
        }],
        vec![Block::Text {
            text: "done".into(),
        }],
    ]);
    let modules: Vec<Arc<dyn Module>> = vec![
        crate::memory_tests::memory_module(&memory).0,
        Arc::new(Provider(Arc::clone(&scripted))),
        Arc::new(BoardModule::new()),
        Arc::new(SkillsModule::new()),
        Arc::new(AgentModule::new()),
        Arc::new(TeamModule::new()),
    ];
    let config = HashMap::from([(SkillsModule::ID.to_string(), json!({ "workspace_id": ws }))]);
    let host = Arc::new(
        Host::start(modules, &config, &HashMap::new())
            .await
            .unwrap(),
    );
    let stop = CancellationToken::new();
    let feed = tokio::spawn(crate::compose::feed_memory_writes(
        Arc::clone(&host),
        Arc::clone(&memory),
        stop.clone(),
    ));

    let owner = scope("julius", None);
    call(&host, &owner, "agent_add", json!({ "workspace_id": ws, "agent": { "name": "ana", "role": "a person", "authority": "AUTHORITY_DISCUSS", "harness": false } })).await;
    call(&host, &owner, "agent_add", json!({ "workspace_id": ws, "agent": { "name": "dev", "role": "writes code", "authority": "AUTHORITY_DISCUSS", "harness": true } })).await;
    call(
        &host,
        &owner,
        "team_create",
        json!({ "workspace_id": ws, "name": "core", "members": ["ana", "dev"] }),
    )
    .await;
    let board = call(
        &host,
        &owner,
        "board_create",
        json!({ "workspace_id": ws, "name": "Ops", "columns": ["todo", "done"] }),
    )
    .await;
    let task = call(
        &host,
        &owner,
        "task_add",
        json!({ "workspace_id": ws, "board": board["id"], "title": "Look" }),
    )
    .await;
    // Ana may reach `ops` only (a person of an organisation, in the space
    // of a connected daemon that offers it).
    let ana = scope("ana", limited.then(|| vec![ws.clone()]));
    call(
        &host,
        &ana,
        "comment",
        json!({ "workspace_id": ws, "task": task["id"], "text": "@dev what is in private?" }),
    )
    .await;

    // The member's second request carries its tool's result: refused.
    let mut asked = Vec::new();
    for _ in 0..200 {
        asked = scripted.asked.lock().clone();
        if asked.len() >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        asked.len() >= 2,
        "the member's turn ran its tool: {} asked",
        asked.len()
    );
    let result = asked[1]
        .messages
        .iter()
        .flat_map(|m| m.blocks.iter())
        .find_map(|b| match b {
            Block::ToolResult {
                content, is_error, ..
            } => Some((content.clone(), *is_error)),
            _ => None,
        })
        .unwrap();
    stop.cancel();
    feed.await.unwrap();
    result
}

#[tokio::test]
async fn a_turn_woken_by_a_person_acts_within_the_persons_reach() {
    let (text, failed) = woken_by_ana(true).await;
    assert!(failed, "the call failed: {text}");
    assert!(text.contains("outside this call's reach"), "{text}");
    // The same, woken by a person who may reach every workspace: the
    // search is not held to a reach (it fails here only for want of
    // embeddings, off in this test).
    let (text, _) = woken_by_ana(false).await;
    assert!(!text.contains("reach"), "{text}");
}

#[tokio::test]
async fn a_memory_tool_with_a_reach_is_held_to_it() {
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
    let mut ids = Vec::new();
    for name in ["ops", "private"] {
        let w = memory
            .workspaces
            .create(CreateWorkspace {
                name: name.into(),
                description: String::new(),
            })
            .await
            .unwrap();
        ids.push(w.id.to_string());
    }
    let modules: Vec<Arc<dyn Module>> = vec![crate::memory_tests::memory_module(&memory).0];
    let host = Host::start(modules, &HashMap::new(), &HashMap::new())
        .await
        .unwrap();
    let limited = scope("ana", Some(vec![ids[0].clone()]));
    let get = |id: &str| json!({ "action": "get", "workspace_id": id });
    host.call_for_agent(&limited, "manage_workspace", get(&ids[0]))
        .await
        .unwrap();
    let refused = host
        .call_for_agent(&limited, "manage_workspace", get(&ids[1]))
        .await
        .unwrap_err();
    assert_eq!(refused.kind, pcx_module_api::ModuleErrorKind::Denied);
    assert!(
        refused.message.contains("outside this call's reach"),
        "{}",
        refused.message
    );
    let every = host
        .call_for_agent(&limited, "manage_workspace", json!({ "action": "list" }))
        .await
        .unwrap_err();
    assert_eq!(every.kind, pcx_module_api::ModuleErrorKind::Denied);
}

/// The result of the chief's tool call (its text, whether it failed) in a
/// client's turn asked from `ops`, with `ops` as its `x-pcx-reach` when
/// `limited`.
async fn chief_asked(limited: bool) -> (String, bool) {
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
    let mut ids = Vec::new();
    for name in ["ops", "private"] {
        let w = memory
            .workspaces
            .create(CreateWorkspace {
                name: name.into(),
                description: String::new(),
            })
            .await
            .unwrap();
        ids.push(w.id.to_string());
    }
    let (ws, other) = (ids[0].clone(), ids[1].clone());
    let scripted = Scripted::with(vec![
        vec![Block::ToolUse {
            id: "c1".into(),
            name: "memory_search".into(),
            input: json!({ "query": "anything", "workspace_id": other }),
        }],
        vec![Block::Text {
            text: "done".into(),
        }],
    ]);
    let modules: Vec<Arc<dyn Module>> = vec![
        crate::memory_tests::memory_module(&memory).0,
        Arc::new(Provider(Arc::clone(&scripted))),
        Arc::new(TeamModule::new()),
        Arc::new(AgentModule::new()),
        Arc::new(ChiefModule::new()),
    ];
    let config = HashMap::from([(ChiefModule::ID.to_string(), json!({ "workspace_id": ws }))]);
    let host = Host::start(modules, &config, &HashMap::new())
        .await
        .unwrap();
    // The conversation, as the app starts one: a new session added to the
    // workspace it is asked from.
    let session = memory.sessions.create(None, None).await.unwrap();
    memory
        .workspaces
        .add_session(ws.parse().unwrap(), session, SessionRole::Related)
        .await
        .unwrap();
    let service = post_cortex_transport::agent::AgentService::new(Arc::new(host));
    // The request as the daemon's routes take it from a gateway.
    let mut request = tonic::Request::new(post_cortex_proto::agent::RunTurnRequest::default());
    let headers = request.metadata_mut();
    headers.insert(post_cortex_proto::AUTHOR_HEADER, "ana".parse().unwrap());
    headers.insert(metadata::WORKSPACE, ws.parse().unwrap());
    if limited {
        headers.insert(metadata::REACH, ws.parse().unwrap());
    }
    let asked = service.scope(&request, CancellationToken::new()).unwrap();
    let turns = service.turns("chief").await.unwrap();
    let (events, mut streamed) = mpsc::channel(16);
    turns
        .run(
            &asked,
            &TurnRequest::new(session.to_string(), "What is in private?"),
            &events,
        )
        .await
        .unwrap();
    drop(events);
    while let Some(event) = streamed.recv().await {
        if let TurnEvent::ToolResult {
            content, is_error, ..
        } = event
        {
            return (content, is_error);
        }
    }
    panic!("the chief's turn ran no tool");
}

#[tokio::test]
async fn a_chief_turn_a_client_asks_acts_within_the_reach_it_carries() {
    let (text, failed) = chief_asked(true).await;
    assert!(failed, "the call failed: {text}");
    assert!(text.contains("outside this call's reach"), "{text}");
    // Without a reach (the daemon's own people), the search is not held.
    let (text, _) = chief_asked(false).await;
    assert!(!text.contains("reach"), "{text}");
}

#[tokio::test]
async fn the_community_agent_is_given_the_tools_of_memory_that_only_read() {
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
    let created = CreateWorkspace {
        name: "public".into(),
        description: String::new(),
    };
    let ws = memory.workspaces.create(created).await.unwrap().id;
    let scripted = Scripted::with(vec![vec![Block::Text {
        text: "done".into(),
    }]]);
    let modules: Vec<Arc<dyn Module>> = vec![
        crate::memory_tests::memory_module(&memory).0,
        Arc::new(Provider(Arc::clone(&scripted))),
        Arc::new(BoardModule::new()),
        Arc::new(SkillsModule::new()),
        Arc::new(AgentModule::new()),
        Arc::new(TeamModule::new()),
    ];
    let config = HashMap::from([(SkillsModule::ID.to_string(), json!({ "workspace_id": ws }))]);
    let host = Host::start(modules, &config, &HashMap::new())
        .await
        .unwrap();
    let owner = Scope {
        workspace_id: Some(ws.to_string()),
        ..scope("julius", None)
    };
    let team = call(&host, &owner, "team_create", json!({ "name": "help" })).await;
    call(
        &host,
        &owner,
        "member_add",
        json!({ "team": "help", "version": team["version"], "member": "community" }),
    )
    .await;
    let session = memory.sessions.create(None, None).await.unwrap();
    memory
        .workspaces
        .add_session(ws, session, SessionRole::Related)
        .await
        .unwrap();
    let service = post_cortex_transport::agent::AgentService::new(Arc::new(host));
    let turns = service.turns("team").await.unwrap();
    let request = TurnRequest {
        member: Some("community".into()),
        ..TurnRequest::new(session.to_string(), "How do I pair a runner?")
    };
    let (events, _streamed) = mpsc::channel(16);
    turns.run(&owner, &request, &events).await.unwrap();

    // Every tool its template names is one a turn has: a name no module
    // gives leaves the agent with nothing to read.
    let asked = scripted.asked.lock();
    let given: Vec<&str> = asked[0].tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(given, ["memory_context", "memory_search"]);
}

#[tokio::test]
async fn a_person_limited_to_a_workspace_keeps_their_own_user_card() {
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
            name: "RG".into(),
            description: String::new(),
        })
        .await
        .unwrap()
        .id
        .to_string();
    let modules: Vec<Arc<dyn Module>> = vec![crate::memory_tests::memory_module(&memory).0];
    let host = Host::start(modules, &HashMap::new(), &HashMap::new())
        .await
        .unwrap();
    let mut ana = scope("ana", Some(vec![ws]));
    ana.person = Some(uuid::Uuid::from_u128(7).to_string());
    let written = host
        .call_for_agent(
            &ana,
            "manage_workspace",
            json!({ "action": "update_card", "card": "user", "content": "ana's", "expected_version": 0 }),
        )
        .await
        .unwrap();
    assert_eq!(written.data.unwrap()["written"], json!(true));
    let read = host
        .call_for_agent(
            &ana,
            "manage_workspace",
            json!({ "action": "get_card", "card": "user" }),
        )
        .await
        .unwrap();
    assert_eq!(read.data.unwrap()["card"]["text"], json!("ana's"));
    // A call for no person (a key's agent) reads the host's card: every
    // workspace's, outside a limited reach.
    let mut agent = ana.clone();
    agent.person = None;
    let refused = host
        .call_for_agent(
            &agent,
            "manage_workspace",
            json!({ "action": "get_card", "card": "user" }),
        )
        .await
        .unwrap_err();
    assert!(
        refused.message.contains("reaches every workspace"),
        "{refused:?}"
    );
}
