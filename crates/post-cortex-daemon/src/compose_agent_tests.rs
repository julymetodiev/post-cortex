// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The agent over memory as the daemon composes it (ARCHITECTURE section
//! 7, the agent): a turn whose model calls a memory RPC, every turn kept
//! in the session's stream with its parent and blocks, and the next turn
//! resumed from it.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use pcx_agent::AgentModule;
use pcx_context::ContextModule;
use pcx_host::Host;
use pcx_module_api::agent::{TurnEvent, TurnRequest, TurnStop};
use pcx_module_api::model::{Block, StopReason};
use pcx_module_api::{Module, Scope};
use post_cortex_core::core::message::{MessageBlock, MessageCursor, MessageOrder};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::compose_scripted_tests::{Provider, Scripted};

fn scope() -> Scope {
    Scope {
        tenant: String::new(),
        profile: String::new(),
        session_id: None,
        workspace_id: None,
        author: Some("julius".into()),
        caller_module: None,
        call_depth: 0,
        turn: None,
        reach: None,
        role: None,
        read_only: false,
        person: None,
        deadline: Instant::now() + Duration::from_secs(20),
        cancel: CancellationToken::new(),
    }
}

#[tokio::test]
async fn a_turn_calls_a_memory_tool_and_its_transcript_resumes_the_next() {
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
    let session = memory.sessions.create(None, None).await.unwrap();
    let scripted = Scripted::with(Vec::from([
        vec![Block::ToolUse {
            id: "c1".into(),
            name: "memory_context".into(),
            input: json!({ "session_id": session.to_string(), "query": "workspaces" }),
        }],
        vec![Block::Text {
            text: "You have no workspaces yet.".into(),
        }],
        vec![Block::Text {
            text: "Still none.".into(),
        }],
    ]));
    let agent = Arc::new(AgentModule::new());
    let modules: Vec<Arc<dyn Module>> = vec![
        crate::memory_tests::memory_module(&memory).0,
        Arc::new(Provider(Arc::clone(&scripted))),
        Arc::new(ContextModule::new()),
        agent.clone(),
    ];
    let _host = Host::start(modules, &HashMap::new(), &HashMap::new())
        .await
        .unwrap();

    let (tx, mut rx) = mpsc::channel(64);
    let turns = agent.turns().unwrap();
    let end = turns
        .run(
            &scope(),
            &TurnRequest::new(session.to_string(), "Which workspaces do I have?"),
            &tx,
        )
        .await
        .unwrap();
    assert_eq!(end.stop, TurnStop::Answered(StopReason::EndTurn));
    drop(tx);
    let mut results = Vec::new();
    while let Some(event) = rx.recv().await {
        if let TurnEvent::ToolResult {
            content, is_error, ..
        } = event
        {
            results.push((content, is_error));
        }
    }
    assert_eq!(results.len(), 1);
    assert!(!results[0].1, "{}", results[0].0);

    // The model was told memory's RPCs as tools, not its MCP tools.
    let told: Vec<String> = scripted.asked.lock()[0]
        .tools
        .iter()
        .map(|t| t.name.clone())
        .collect();
    assert!(told.contains(&"memory_context".to_string()), "{told:?}");
    assert!(!told.contains(&"manage_workspace".to_string()));

    // The prompt, the call, its result, the answer and how the turn ended:
    // kept whole, chained.
    let stream = memory
        .messages
        .page(session, MessageCursor::From(0), 10, MessageOrder::Asc)
        .await
        .unwrap();
    assert_eq!(stream.len(), 5);
    assert!(
        matches!(&stream[1].blocks[0], MessageBlock::ToolUse { name, .. } if name == "memory_context")
    );
    assert!(
        matches!(&stream[2].blocks[0], MessageBlock::ToolResult { tool_use_id, .. } if tool_use_id == "c1")
    );
    for pair in stream.windows(2) {
        assert_eq!(pair[1].parent.as_deref(), Some(pair[0].turn_id.as_str()));
    }
    assert_eq!(stream[3].text, "You have no workspaces yet.");
    assert!(matches!(&stream[4].blocks[0], MessageBlock::End { stop, .. } if stop == "end_turn"));

    // The next turn starts from the stream.
    let (tx, _rx) = mpsc::channel(64);
    turns
        .run(
            &scope(),
            &TurnRequest::new(session.to_string(), "And now?"),
            &tx,
        )
        .await
        .unwrap();
    assert_eq!(scripted.asked.lock()[2].messages.len(), 5);
}

/// `RunTurn` over gRPC, as the daemon serves it: the turn's events in
/// order, then its end; without an agent the call is refused.
#[tokio::test]
async fn run_turn_streams_a_turn_over_grpc() {
    use post_cortex_proto::agent::RunTurnRequest;
    use post_cortex_proto::agent::agent_client::AgentClient;
    use post_cortex_proto::agent::turn_reply::Event as Reply;

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
    let session = memory.sessions.create(None, None).await.unwrap();
    let scripted = Scripted::with(Vec::from([
        vec![Block::ToolUse {
            id: "c1".into(),
            name: "memory_context".into(),
            input: json!({ "session_id": session.to_string(), "query": "workspaces" }),
        }],
        vec![Block::Text {
            text: "None yet.".into(),
        }],
    ]));
    let (memory_module, kinds) = crate::memory_tests::memory_module(&memory);
    let modules: Vec<Arc<dyn Module>> = vec![
        memory_module,
        Arc::new(Provider(scripted)),
        Arc::new(AgentModule::new()),
    ];
    let host = Arc::new(
        Host::start(modules, &HashMap::new(), &HashMap::new())
            .await
            .unwrap(),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let routes = crate::compose::grpc_routes(Arc::clone(&memory), kinds, host, None, None);
    let served = tokio::spawn(
        tonic::transport::Server::builder()
            .add_routes(routes)
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener)),
    );

    let mut client = AgentClient::connect(format!("http://{addr}"))
        .await
        .unwrap();
    let mut replies = client
        .run_turn(RunTurnRequest {
            session_id: session.to_string(),
            prompt: "Which workspaces?".into(),
            ..RunTurnRequest::default()
        })
        .await
        .unwrap()
        .into_inner();
    let mut kinds = Vec::new();
    while let Some(reply) = replies.message().await.unwrap() {
        kinds.push(match reply.event.unwrap() {
            Reply::Text(text) => format!("text {text}"),
            Reply::ToolCall(call) => format!("call {}", call.name),
            Reply::ToolResult(result) => format!("result {}", result.is_error),
            Reply::End(end) => format!("end {} {}", end.stop, end.steps),
            Reply::Started(_) => "started".to_string(),
            other => format!("{other:?}"),
        });
    }
    assert_eq!(
        kinds,
        [
            "started",
            "call memory_context",
            "result false",
            "end end_turn 2"
        ]
    );
    served.abort();
}

/// The context engine over memory: it recalls a session's records for a
/// prompt, and keeps a summary as a record of the session.
#[tokio::test]
async fn the_context_engine_recalls_and_keeps_its_summary_in_memory() {
    use pcx_module_api::model::{ModelMessage, Role};

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
    let session = memory.sessions.create(None, None).await.unwrap();
    let scripted = Scripted::with(Vec::from([vec![Block::Text {
        text: "They planned the deploy.".into(),
    }]]));
    let context = Arc::new(ContextModule::new());
    let modules: Vec<Arc<dyn Module>> = vec![
        crate::memory_tests::memory_module(&memory).0,
        Arc::new(Provider(scripted)),
        context.clone(),
    ];
    let _host = Host::start(modules, &HashMap::new(), &HashMap::new())
        .await
        .unwrap();
    let engine = context.context_engine().unwrap();
    let s = session.to_string();

    // Nothing recorded yet: nothing recalled.
    assert_eq!(engine.recall(&scope(), &s, "the deploy").await.unwrap(), "");

    let said = vec![ModelMessage {
        role: Role::User,
        blocks: vec![Block::Text {
            text: "Plan the deploy".into(),
        }],
    }];
    let summary = engine
        .summarize(&scope(), &s, "glm-5.3", &said)
        .await
        .unwrap();
    assert_eq!(summary, "They planned the deploy.");
    let kept = memory.sessions.get(session).await.unwrap();
    assert_eq!(
        kept.records().len(),
        1,
        "the summary is a record of the session"
    );
}

/// One real turn of the agent with Z.AI, only when asked: the model calls
/// a memory tool and answers from its result.
///
/// ```sh
/// PCX_ZAI_KEY_FILE=~/.axon/zai_auth.json cargo test -p post-cortex-daemon --lib live_agent -- --ignored --nocapture
/// ```
#[tokio::test]
#[ignore = "calls Z.AI: set PCX_ZAI_KEY_FILE and pass --ignored"]
async fn live_agent_turn_with_zai() {
    let file = std::env::var("PCX_ZAI_KEY_FILE").expect("PCX_ZAI_KEY_FILE names the key file");
    let stored: Value = serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
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
    let session = memory.sessions.create(None, None).await.unwrap();
    let agent = Arc::new(AgentModule::new());
    let modules: Vec<Arc<dyn Module>> = vec![
        crate::memory_tests::memory_module(&memory).0,
        Arc::new(pcx_models::ModelsModule::new()),
        Arc::new(ContextModule::new()),
        agent.clone(),
    ];
    let config = HashMap::from([(
        "models".to_string(),
        json!({ "zai": { "api_key": stored["api_key"] } }),
    )]);
    let policies = HashMap::from([(
        "models".to_string(),
        pcx_host::sandbox_policy(&[pcx_host::Grant::Network], &[], &[]),
    )]);
    let _host = Host::start(modules, &config, &policies).await.unwrap();
    let (tx, mut rx) = mpsc::channel(256);
    let mut long = scope();
    long.deadline = Instant::now() + Duration::from_secs(180);
    let printer = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            eprintln!("{event:?}");
        }
    });
    let end = agent
        .turns()
        .unwrap()
        .run(
            &long,
            &TurnRequest::new(
                session.to_string(),
                "Use manage_workspace (action list) to tell me how many workspaces I have.",
            ),
            &tx,
        )
        .await
        .unwrap();
    drop(tx);
    printer.await.unwrap();
    eprintln!("end: {end:?}");
    assert_eq!(end.stop, TurnStop::Answered(StopReason::EndTurn));
    assert!(end.steps >= 2, "the model called the tool, then answered");
}

/// Five real turns of one session with Z.AI, only when asked: from the
/// second on, the provider's cache serves most of each request's input
/// (the system prompt, the tools and the conversation stay the same from
/// turn to turn; recall goes with the prompt).
///
/// ```sh
/// PCX_ZAI_KEY_FILE=~/.axon/zai_auth.json cargo test -p post-cortex-daemon --lib live_turns_reuse -- --ignored --nocapture
/// ```
#[tokio::test]
#[ignore = "calls Z.AI: set PCX_ZAI_KEY_FILE and pass --ignored"]
async fn live_turns_reuse_the_providers_cache() {
    let file = std::env::var("PCX_ZAI_KEY_FILE").expect("PCX_ZAI_KEY_FILE names the key file");
    let stored: Value = serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
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
    let session = memory.sessions.create(None, None).await.unwrap();
    let agent = Arc::new(AgentModule::new());
    let modules: Vec<Arc<dyn Module>> = vec![
        crate::memory_tests::memory_module(&memory).0,
        Arc::new(pcx_models::ModelsModule::new()),
        Arc::new(ContextModule::new()),
        agent.clone(),
    ];
    let config = HashMap::from([(
        "models".to_string(),
        json!({ "zai": { "api_key": stored["api_key"] } }),
    )]);
    let policies = HashMap::from([(
        "models".to_string(),
        pcx_host::sandbox_policy(&[pcx_host::Grant::Network], &[], &[]),
    )]);
    let _host = Host::start(modules, &config, &policies).await.unwrap();
    let turns = agent.turns().unwrap();
    let prompts = [
        "Use manage_workspace (action list) and tell me how many workspaces I have.",
        "Create a workspace named cache-test with manage_workspace, then say done.",
        "List the workspaces again and name them.",
        "In one sentence: what did we do in this conversation?",
        "Say goodbye in three words.",
    ];
    let mut served = Vec::new();
    for (n, prompt) in prompts.iter().enumerate() {
        let mut long = scope();
        long.deadline = Instant::now() + Duration::from_secs(180);
        let (tx, _rx) = mpsc::channel(256);
        let end = turns
            .run(&long, &TurnRequest::new(session.to_string(), *prompt), &tx)
            .await
            .unwrap();
        let input = end.usage.input + end.usage.cache_read + end.usage.cache_write;
        let share = if input == 0 {
            0.0
        } else {
            end.usage.cache_read as f64 / input as f64
        };
        eprintln!(
            "turn {}: {:?}, {} steps, input {input} (cached {}, {:.0}%), output {}",
            n + 1,
            end.stop,
            end.steps,
            end.usage.cache_read,
            share * 100.0,
            end.usage.output
        );
        assert_eq!(end.stop, TurnStop::Answered(StopReason::EndTurn));
        served.push(share);
    }
    for (n, share) in served.iter().enumerate().skip(1) {
        assert!(
            *share > 0.8,
            "turn {}: the cache served {:.0}% of the input",
            n + 1,
            share * 100.0
        );
    }
}
