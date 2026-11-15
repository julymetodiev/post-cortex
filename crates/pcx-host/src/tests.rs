// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

use std::time::{Duration, Instant};

use async_trait::async_trait;
use pcx_module_api::view::HTML_MIME;
use pcx_module_api::{
    API_VERSION, ApiVersion, FailurePolicy, HookSpecificOutput, HostContext, Manifest, ModuleKind,
    Outcome, Resource, Subscription,
};
use tokio_util::sync::CancellationToken;

/// What a fake module does when called.
#[derive(Clone, Copy)]
enum Act {
    Answer,
    Fail,
    Panic,
    Hang,
    Block,
}

struct Fake {
    manifest: Manifest,
    tool: String,
    act: Act,
    fails_start: bool,
    /// How long its start takes.
    starts_in: Duration,
    stopped: Arc<std::sync::atomic::AtomicBool>,
}

fn fake(id: &str, kind: ModuleKind, act: Act, policy: FailurePolicy) -> Fake {
    Fake {
        manifest: Manifest {
            emits: Vec::new(),
            follows: Vec::new(),
            workflow_templates: Vec::new(),
            id: id.to_string(),
            version: "0.1.0".to_string(),
            api: Some(API_VERSION),
            kind: kind.into(),
            events: vec![Subscription {
                kind: EventKind::TurnStart.into(),
                deadline_ms: 50,
                policy: policy.into(),
                memory: None,
            }],
            capabilities: vec![],
            filesystem: vec![],
            views: vec![],
            document_kinds: vec![],
            uses: Vec::new(),
            prompts: false,
            turns: false,
            services: Vec::new(),
            descriptors: Vec::new(),
            rpcs: Vec::new(),
            config_type: String::new(),
            description: String::new(),
        },
        tool: format!("{id}_echo"),
        act,
        fails_start: false,
        starts_in: Duration::ZERO,
        stopped: Arc::default(),
    }
}

impl Fake {
    async fn act(&self) -> Result<(), ModuleError> {
        match self.act {
            Act::Answer | Act::Block => Ok(()),
            Act::Fail => Err(ModuleError::new(ModuleErrorKind::Unavailable, "down")),
            Act::Panic => panic!("fake module panics"),
            Act::Hang => {
                tokio::time::sleep(Duration::from_secs(60)).await;
                Ok(())
            }
        }
    }
}

#[async_trait]
impl Module for Fake {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    async fn start(&self, _ctx: HostContext) -> Result<(), ModuleError> {
        tokio::time::sleep(self.starts_in).await;
        if self.fails_start {
            return Err(ModuleError::new(ModuleErrorKind::Unavailable, "no backend"));
        }
        Ok(())
    }

    /// The echo tool, and one only its views call.
    fn mcp_tools(&self) -> Vec<ToolSpec> {
        let tool = |name: String, description: &str, visibility| ToolSpec {
            name,
            description: description.to_string(),
            input_schema: serde_json::Map::new(),
            visibility,
            effect: pcx_module_api::Effect::Read,
        };
        use pcx_module_api::Visibility::{App, Both};
        vec![
            tool(self.tool.clone(), "Echo the input", Both),
            tool(format!("{}_rows", self.manifest.id), "Rows for a view", App),
        ]
    }

    async fn call_mcp(
        &self,
        _scope: &Scope,
        _tool: &str,
        input: Value,
    ) -> Result<ToolOutput, ModuleError> {
        self.act().await?;
        Ok(ToolOutput {
            text: "echo".to_string(),
            data: Some(input),
            failed: false,
        })
    }

    async fn on_event(&self, _scope: &Scope, _event: &Event) -> Result<Outcome, ModuleError> {
        self.act().await?;
        Ok(Outcome {
            decision: if matches!(self.act, Act::Block) {
                "block"
            } else {
                ""
            }
            .to_string(),
            reason: format!("{} says no", self.manifest.id),
            hook_specific_output: Some(HookSpecificOutput {
                hook_event_name: "UserPromptSubmit".to_string(),
                additional_context: format!("from {}", self.manifest.id),
            }),
        })
    }

    /// The HTML of a view it declares; `<p>{uri}</p>`.
    async fn resource(&self, uri: &str) -> Result<Resource, ModuleError> {
        self.act().await?;
        let declared = self
            .manifest
            .views
            .iter()
            .any(|v| v.body == Some(pcx_module_api::view::Body::ResourceUri(uri.to_string())));
        if !declared {
            return Err(ModuleError::new(ModuleErrorKind::NotFound, uri));
        }
        Ok(Resource {
            uri: uri.to_string(),
            mime_type: HTML_MIME.to_string(),
            text: format!("<p>{uri}</p>"),
        })
    }

    async fn stop(&self) -> Result<(), ModuleError> {
        self.stopped
            .store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
}

fn scope(within: Duration) -> Scope {
    Scope {
        tenant: String::new(),
        profile: "default".to_string(),
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
        deadline: Instant::now() + within,
        cancel: CancellationToken::new(),
    }
}

fn prompt() -> Event {
    Event {
        hook_event_name: "UserPromptSubmit".to_string(),
        prompt: "hello".to_string(),
        ..Event::default()
    }
}

async fn host(modules: Vec<Fake>) -> Result<Host, HostError> {
    let modules = modules
        .into_iter()
        .map(|m| Arc::new(m) as Arc<dyn Module>)
        .collect();
    Host::start(modules, &HashMap::new(), &HashMap::new()).await
}

fn tool(act: Act) -> Fake {
    fake("tool", ModuleKind::Tool, act, FailurePolicy::FailOpen)
}

#[tokio::test]
async fn a_second_memory_module_is_refused() {
    let refused = host(vec![
        fake(
            "a",
            ModuleKind::Memory,
            Act::Answer,
            FailurePolicy::FailOpen,
        ),
        fake(
            "b",
            ModuleKind::Memory,
            Act::Answer,
            FailurePolicy::FailOpen,
        ),
    ])
    .await;
    assert!(matches!(
        refused,
        Err(HostError::SlotTaken { slot: "memory", ref existing, ref module })
            if existing == "a" && module == "b"
    ));
}

#[tokio::test]
async fn two_modules_naming_a_tool_on_one_rpc_are_refused() {
    const ADD: &str = "/acme.notes.v1.Notes/Add";
    let serving = |id: &str, tool: &str| {
        let mut module = fake(id, ModuleKind::Tool, Act::Answer, FailurePolicy::FailOpen);
        module.manifest.services = vec!["acme.notes.v1.Notes".to_string()];
        module.manifest.rpcs = vec![pcx_module_api::Rpc {
            method: ADD.to_string(),
            tool: tool.to_string(),
            visibility: Vec::new(),
            effect: pcx_module_api::Effect::Write.into(),
        }];
        module
    };
    let refused = host(vec![serving("a", "note_add"), serving("b", "b_note_add")]).await;
    assert!(matches!(
        refused,
        Err(HostError::DuplicateToolPath { ref path, ref first, ref module })
            if path == ADD && first == "a" && module == "b"
    ));
    // Several modules may serve one service when one names the tool
    // (docs/host.md, a service several modules serve).
    assert!(host(vec![serving("a", ""), serving("b", "")]).await.is_ok());
}

#[tokio::test]
async fn bad_manifests_are_refused() {
    let mut newer = tool(Act::Answer);
    newer.manifest.api = Some(ApiVersion { major: 2, minor: 0 });
    assert!(matches!(
        host(vec![newer]).await,
        Err(HostError::IncompatibleApi { .. })
    ));

    let kindless = fake(
        "x",
        ModuleKind::Unspecified,
        Act::Answer,
        FailurePolicy::FailOpen,
    );
    assert!(matches!(
        host(vec![kindless]).await,
        Err(HostError::InvalidManifest { .. })
    ));

    let mut unset_event = tool(Act::Answer);
    unset_event.manifest.events[0].kind = EventKind::Unspecified.into();
    assert!(matches!(
        host(vec![unset_event]).await,
        Err(HostError::InvalidManifest { .. })
    ));

    assert!(matches!(
        host(vec![tool(Act::Answer), tool(Act::Answer)]).await,
        Err(HostError::DuplicateId(id)) if id == "tool"
    ));
}

#[tokio::test]
async fn a_failed_start_stops_the_modules_started_before_it() {
    let first = tool(Act::Answer);
    let stopped = Arc::clone(&first.stopped);
    let mut second = fake(
        "second",
        ModuleKind::Tool,
        Act::Answer,
        FailurePolicy::FailOpen,
    );
    second.fails_start = true;
    let failed = host(vec![first, second]).await;
    assert!(matches!(failed, Err(HostError::StartFailed { ref module, .. }) if module == "second"));
    assert!(stopped.load(std::sync::atomic::Ordering::SeqCst));
}

#[tokio::test]
async fn two_modules_with_one_tool_name_are_refused() {
    let first = fake("a", ModuleKind::Tool, Act::Answer, FailurePolicy::FailOpen);
    let stopped = Arc::clone(&first.stopped);
    let mut second = fake("b", ModuleKind::Tool, Act::Answer, FailurePolicy::FailOpen);
    second.tool = "a_echo".to_string();
    let refused = host(vec![first, second]).await;
    assert!(matches!(
        refused,
        Err(HostError::DuplicateTool { ref tool, ref first, ref module })
            if tool == "a_echo" && first == "a" && module == "b"
    ));
    assert!(stopped.load(std::sync::atomic::Ordering::SeqCst));
}

#[tokio::test]
async fn a_call_returns_the_module_output() {
    let host = host(vec![tool(Act::Answer)]).await.unwrap();
    let out = host
        .call_for_agent(&scope(Duration::from_secs(1)), "tool_echo", Value::from(7))
        .await
        .unwrap();
    assert_eq!(out.data, Some(Value::from(7)));
    assert!(host.has_tool("tool_echo"));
    assert_eq!(host.tools().len(), 1);
    let unknown = host
        .call_for_agent(&scope(Duration::from_secs(1)), "nope", Value::Null)
        .await;
    assert_eq!(unknown.unwrap_err().kind, ModuleErrorKind::NotFound);
}

#[tokio::test]
async fn a_panic_is_caught_and_counted() {
    let host = host(vec![tool(Act::Panic)]).await.unwrap();
    let panicked = host
        .call_for_agent(&scope(Duration::from_secs(1)), "tool_echo", Value::Null)
        .await;
    assert_eq!(panicked.unwrap_err().kind, ModuleErrorKind::Internal);
    let health = &host.health()[0];
    assert_eq!((health.calls, health.panics, health.errors), (1, 1, 0));
}

#[tokio::test]
async fn a_hung_call_ends_at_its_deadline() {
    let host = host(vec![tool(Act::Hang)]).await.unwrap();
    let late = host
        .call_for_agent(&scope(Duration::from_millis(20)), "tool_echo", Value::Null)
        .await;
    assert_eq!(late.unwrap_err().kind, ModuleErrorKind::DeadlineExceeded);
    assert_eq!(host.health()[0].timeouts, 1);
}

#[tokio::test]
async fn a_cancelled_call_stops_and_is_not_the_module_failure() {
    let host = host(vec![tool(Act::Hang)]).await.unwrap();
    let scope = scope(Duration::from_secs(30));
    scope.cancel.cancel();
    let cancelled = host.call_for_agent(&scope, "tool_echo", Value::Null).await;
    assert_eq!(
        cancelled.unwrap_err().kind,
        ModuleErrorKind::DeadlineExceeded
    );
    let health = &host.health()[0];
    assert_eq!(
        (health.cancelled, health.errors, health.timeouts),
        (1, 0, 0)
    );
}

#[test]
fn percentiles_come_from_the_buckets() {
    let stats = stats::Stats::default();
    for ms in [1, 1, 1, 30, 3_000] {
        stats.record(Duration::from_millis(ms), stats::Ended::Done);
    }
    let health = stats.health("m", false);
    assert_eq!(health.p50, Some(Duration::from_millis(1)));
    assert_eq!(health.p99, Some(Duration::from_millis(5_000)));
    assert_eq!(stats::Stats::default().health("m", false).p50, None);
}

mod documents;
mod events;
mod faults;
mod models;
mod prompts;
mod rpc_tools;
mod settings;
mod turns;
mod views;
