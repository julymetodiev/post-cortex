// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Modules together end to end: the board keeps its documents in memory
//! through memory's `pcx.v1` RPCs on its host channel; memory takes the
//! kinds from the running modules and the writer from the host, so only
//! the board writes them and anyone reads them (memory's MCP tools); a
//! module that follows them hears of each change.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use parking_lot::Mutex;
use pcx_board::BoardModule;
use pcx_host::Host;
use pcx_module_api::{
    API_VERSION, Event, EventKind, HostContext, Manifest, MemoryFilter, Module, ModuleError,
    ModuleErrorKind, ModuleKind, Outcome, Resource, Scope, Subscription, json as wire_json,
};
use post_cortex_memory::capabilities::CreateWorkspace;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

/// A module that follows the board's tasks and keeps what it heard.
struct Watcher {
    manifest: Manifest,
    heard: Arc<Mutex<Vec<Value>>>,
}

impl Watcher {
    fn new(heard: Arc<Mutex<Vec<Value>>>) -> Self {
        Self {
            manifest: Manifest {
                id: "watcher".to_string(),
                version: "0.1.0".to_string(),
                api: Some(API_VERSION),
                kind: ModuleKind::Tool.into(),
                events: vec![Subscription {
                    kind: EventKind::MemoryWrite.into(),
                    deadline_ms: 0,
                    policy: pcx_module_api::FailurePolicy::FailOpen.into(),
                    memory: Some(MemoryFilter {
                        document_kinds: vec!["board/task".to_string()],
                        ..MemoryFilter::default()
                    }),
                }],
                ..Manifest::default()
            },
            heard,
        }
    }
}

#[async_trait]
impl Module for Watcher {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    async fn start(&self, _ctx: HostContext) -> Result<(), ModuleError> {
        Ok(())
    }

    async fn on_event(&self, _scope: &Scope, event: &Event) -> Result<Outcome, ModuleError> {
        let payload = event.payload.clone().map(wire_json::object_from_wire);
        self.heard
            .lock()
            .push(Value::Object(payload.unwrap_or_default()));
        Ok(Outcome::default())
    }

    async fn resource(&self, uri: &str) -> Result<Resource, ModuleError> {
        Err(ModuleError::new(ModuleErrorKind::NotFound, uri))
    }

    async fn stop(&self) -> Result<(), ModuleError> {
        Ok(())
    }
}

fn scope() -> Scope {
    Scope {
        tenant: String::new(),
        profile: String::new(),
        session_id: None,
        workspace_id: None,
        author: Some("claude-code".to_string()),
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

/// The data of MCP tool `tool` called with `input`.
async fn call(host: &Host, tool: &str, input: Value) -> Value {
    host.call_for_agent(&scope(), tool, input)
        .await
        .unwrap()
        .data
        .unwrap()
}

#[tokio::test]
async fn a_module_keeps_its_documents_in_memory() {
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
            name: "together".into(),
            description: String::new(),
        })
        .await
        .unwrap()
        .id
        .to_string();
    let heard = Arc::new(Mutex::new(Vec::new()));
    let modules: Vec<Arc<dyn Module>> = vec![
        crate::memory_tests::memory_module(&memory).0,
        Arc::new(BoardModule::new()),
        Arc::new(Watcher::new(Arc::clone(&heard))),
    ];
    let host = Arc::new(
        Host::start(modules, &HashMap::new(), &HashMap::new())
            .await
            .unwrap(),
    );
    // The feed follows the writes from here on: the task below is written
    // before the feed first runs, and is heard.
    let stop = CancellationToken::new();
    let following = crate::compose::feed_memory_writes(
        Arc::clone(&host),
        Arc::clone(&memory),
        stop.clone(),
    );

    let board = call(
        &host,
        "board_create",
        json!({"workspace_id": ws, "name": "Sprint", "columns": ["todo", "done"]}),
    )
    .await;
    let task = call(
        &host,
        "task_add",
        json!({"workspace_id": ws, "board": board["id"], "title": "Ship W23"}),
    )
    .await;
    let id = task["id"].as_str().unwrap().to_string();
    let feed = tokio::spawn(following);

    // The watcher hears of the new task.
    let deadline = Instant::now() + Duration::from_secs(10);
    while heard.lock().is_empty() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        heard.lock().clone(),
        vec![json!({
            "change": "document", "workspace_id": ws, "id": id,
            "kind": "board/task", "version": 1, "deleted": false
        })]
    );

    // Anyone reads it, through memory's MCP tools.
    let got = call(
        &host,
        "manage_document",
        json!({"action": "get", "workspace_id": ws, "document_id": id}),
    )
    .await;
    assert_eq!(got["document"]["kind"], json!("board/task"));
    assert_eq!(got["document"]["author"], json!("claude-code"));
    let listed = call(
        &host,
        "manage_document",
        json!({"action": "list", "workspace_id": ws, "kind": "board/task"}),
    )
    .await;
    assert_eq!(listed["total"], json!(1));

    // An agent is not the board: it neither writes nor deletes the board's
    // tasks.
    let put = json!({
        "action": "put", "workspace_id": ws, "kind": "board/task", "document_id": id,
        "data": {"title": "mine"}, "expected_version": 1
    });
    let refused = host
        .call_for_agent(&scope(), "manage_document", put)
        .await
        .unwrap_err();
    assert_eq!(refused.kind, ModuleErrorKind::Denied, "{refused:?}");
    let delete = json!({"action": "delete", "workspace_id": ws, "document_id": id});
    let refused = host
        .call_for_agent(&scope(), "manage_document", delete)
        .await
        .unwrap_err();
    assert_eq!(refused.kind, ModuleErrorKind::Denied, "{refused:?}");
    stop.cancel();
    feed.await.unwrap();
    host.stop().await;
}
