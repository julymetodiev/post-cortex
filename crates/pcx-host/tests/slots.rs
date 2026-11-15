// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The agent's slots across processes, end to end. This binary is the test
//! and every module: started with `PCX_MODULE_SOCKET` it serves the module
//! its first argument names (a model provider, a context engine, an agent,
//! a member that runs the agent's turns); without it, it launches them all
//! as processes through one host and checks a member's turn crossing
//! every hop (member → host → agent → host → provider and context), a
//! client's turn, a cancel that still reads the turn's end, a member's turn
//! whose tools are the member's reach (the host's token crossing the
//! agent's process), a module process's own turns (`Manifest.turns`), and a
//! module that does not use the agent refused its turns.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use parking_lot::Mutex;
use pcx_host::{Host, Origin, ProcessModule, ProcessSpec};
use pcx_module_api::agent::{TurnEnd, TurnEvent, TurnRequest, TurnStop, Turns};
use pcx_module_api::context::ContextEngine;
use pcx_module_api::grpc::{self, GrpcChannel};
use pcx_module_api::model::ModelProvider;
use pcx_module_api::{
    API_VERSION, Event, EventKind, HostContext, Manifest, Module, ModuleError, ModuleErrorKind,
    ModuleHost, ModuleKind, Outcome, Resource, SOCKET_ENV, Scope, serve,
};
use post_cortex_proto::module as wire;
use post_cortex_proto::module::hooks_client::HooksClient;
use post_cortex_proto::module::hooks_server::{Hooks, HooksServer};
use post_cortex_proto::skills;
use post_cortex_proto::skills::skills_client::SkillsClient;
use post_cortex_proto::skills::skills_server::{Skills, SkillsServer};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

mod slot_ports;

use slot_ports::{Agent, Recall, STARTED_MODEL, Scripted};

fn manifest(id: &str, kind: ModuleKind, uses: &[&str]) -> Manifest {
    Manifest {
        id: id.to_string(),
        version: "1.0.0".to_string(),
        api: Some(API_VERSION),
        kind: kind.into(),
        uses: uses.iter().map(|u| (*u).to_string()).collect(),
        ..Manifest::default()
    }
}

/// A module's handle, once it starts.
type Handle = Arc<Mutex<Option<Arc<dyn ModuleHost>>>>;

/// A module of these tests: its manifest, its handle and its slot's port.
struct Test {
    manifest: Manifest,
    host: Handle,
    models: Option<Arc<Scripted>>,
    context: Option<Arc<dyn ContextEngine>>,
    turns: Option<Arc<dyn Turns>>,
}

impl Test {
    fn new(manifest: Manifest) -> Self {
        Self {
            manifest,
            host: Arc::default(),
            models: None,
            context: None,
            turns: None,
        }
    }
}

/// The handle in `host`.
fn handle(host: &Handle) -> Result<Arc<dyn ModuleHost>, ModuleError> {
    host.lock()
        .clone()
        .ok_or_else(|| ModuleError::new(ModuleErrorKind::FailedPrecondition, "not started"))
}

/// A turn of the agent `host` asks for `prompt`: its answer, its events and
/// steps; cancelled at its first event when `cancel`.
async fn ask(
    host: &Handle,
    scope: &Scope,
    prompt: &str,
    cancel: bool,
) -> Result<(TurnEnd, usize), ModuleError> {
    let turns = handle(host)?.turns().await?;
    let request = TurnRequest {
        instructions: "be brief".into(),
        ..TurnRequest::new("s1", prompt)
    };
    let (tx, mut rx) = mpsc::channel(16);
    if !cancel {
        let end = turns.run(scope, &request, &tx).await?;
        drop(tx);
        let mut events = 0;
        while rx.recv().await.is_some() {
            events += 1;
        }
        return Ok((end, events));
    }
    let mut turn_scope = scope.clone();
    turn_scope.cancel = CancellationToken::new();
    let cancelled = turn_scope.cancel.clone();
    let watcher = tokio::spawn(async move {
        if rx.recv().await.is_some() {
            cancelled.cancel();
        }
    });
    let end = turns.run(&turn_scope, &request, &tx).await?;
    watcher
        .await
        .map_err(|e| ModuleError::new(ModuleErrorKind::Internal, e.to_string()))?;
    Ok((end, 1))
}

/// The member's `ask` tool, its one RPC: a turn of the agent for the
/// event's prompt (cancelled at its first event for a `TURN_END` event),
/// answered as its answer and `<events> <steps> <input> <cancelled>`.
struct Member(Handle);

#[tonic::async_trait]
impl Hooks for Member {
    async fn dispatch(
        &self,
        request: tonic::Request<wire::DispatchRequest>,
    ) -> Result<tonic::Response<wire::DispatchResponse>, tonic::Status> {
        let scope = Scope::of(&request)?;
        let request = request.into_inner();
        let prompt = request.event.map(|e| e.prompt).unwrap_or_default();
        let cancel = request.kind == i32::from(EventKind::TurnEnd);
        let (end, events) = ask(&self.0, &scope, &prompt, cancel).await?;
        Ok(tonic::Response::new(wire::DispatchResponse {
            outcome: Some(Outcome {
                decision: end.answer,
                reason: format!(
                    "{events} {} {} {}",
                    end.steps,
                    end.usage.input,
                    end.stop == TurnStop::Cancelled
                ),
                hook_specific_output: None,
            }),
            error: None,
        }))
    }
}

/// The outsider's `outsider_ask` tool, its one RPC: a turn of the agent,
/// which the host refuses it (it does not use the agent).
struct Outsider(Handle);

#[tonic::async_trait]
impl Skills for Outsider {
    async fn add_skill(
        &self,
        _: tonic::Request<skills::AddSkillRequest>,
    ) -> Result<tonic::Response<skills::SkillRecord>, tonic::Status> {
        Err(tonic::Status::unimplemented("AddSkill"))
    }

    async fn update_skill(
        &self,
        _: tonic::Request<skills::UpdateSkillRequest>,
    ) -> Result<tonic::Response<skills::SkillRecord>, tonic::Status> {
        Err(tonic::Status::unimplemented("UpdateSkill"))
    }

    async fn list_skills(
        &self,
        request: tonic::Request<skills::ListSkillsRequest>,
    ) -> Result<tonic::Response<skills::SkillList>, tonic::Status> {
        let scope = Scope::of(&request)?;
        ask(&self.0, &scope, "hi", false).await?;
        Ok(tonic::Response::new(skills::SkillList::default()))
    }

    async fn get_skill(
        &self,
        _: tonic::Request<skills::GetSkillRequest>,
    ) -> Result<tonic::Response<skills::SkillRecord>, tonic::Status> {
        Err(tonic::Status::unimplemented("GetSkill"))
    }
}

#[async_trait]
impl Module for Test {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    async fn start(&self, ctx: HostContext) -> Result<(), ModuleError> {
        *self.host.lock() = Some(ctx.host);
        if let Some(models) = &self.models {
            models
                .started
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
        Ok(())
    }

    async fn stop(&self) -> Result<(), ModuleError> {
        Ok(())
    }

    async fn on_event(&self, _scope: &Scope, _event: &Event) -> Result<Outcome, ModuleError> {
        Ok(Outcome::default())
    }

    async fn resource(&self, uri: &str) -> Result<Resource, ModuleError> {
        Err(ModuleError::new(ModuleErrorKind::NotFound, uri))
    }

    fn model_provider(&self) -> Option<Arc<dyn ModelProvider>> {
        self.models
            .clone()
            .map(|models| models as Arc<dyn ModelProvider>)
    }

    fn context_engine(&self) -> Option<Arc<dyn ContextEngine>> {
        self.context.clone()
    }

    fn turns(&self) -> Option<Arc<dyn Turns>> {
        self.turns.clone()
    }

    fn services(&self) -> Option<GrpcChannel> {
        let host = Arc::clone(&self.host);
        let routes = match self.manifest.id.as_str() {
            "member" => tonic::service::Routes::new(HooksServer::new(Member(host))),
            "outsider" => tonic::service::Routes::new(SkillsServer::new(Outsider(host))),
            _ => return None,
        };
        Some(grpc::routes(routes))
    }
}

/// The module the first argument names.
fn module(which: &str) -> Test {
    match which {
        "provider" => Test {
            models: Some(Arc::new(Scripted::default())),
            ..Test::new(manifest("provider", ModuleKind::ModelProvider, &[]))
        },
        "context" => Test {
            context: Some(Arc::new(Recall)),
            ..Test::new(manifest("context", ModuleKind::ContextEngine, &[]))
        },
        "speaker" => {
            let mut declared = manifest("speaker", ModuleKind::Tool, &[]);
            declared.turns = true;
            let test = Test::new(declared);
            Test {
                turns: Some(Arc::new(Agent {
                    host: Arc::clone(&test.host),
                })),
                ..test
            }
        }
        "agent" => {
            let test = Test::new(manifest(
                "agent",
                ModuleKind::Agent,
                &["member", "outsider"],
            ));
            Test {
                turns: Some(Arc::new(Agent {
                    host: Arc::clone(&test.host),
                })),
                ..test
            }
        }
        "member" => {
            let mut serving = manifest("member", ModuleKind::Tool, &["agent", "outsider"]);
            serving.services = vec!["pcx.module.v1.Hooks".to_string()];
            // The chief's descriptors hold the module protocol's.
            serving.descriptors = post_cortex_proto::CHIEF_DESCRIPTOR_SET.to_vec();
            serving.rpcs = vec![wire::Rpc {
                method: "/pcx.module.v1.Hooks/Dispatch".to_string(),
                tool: "ask".to_string(),
                visibility: Vec::new(),
                effect: post_cortex_proto::module::Effect::Read.into(),
            }];
            Test::new(serving)
        }
        _ => {
            let mut serving = manifest("outsider", ModuleKind::Tool, &[]);
            serving.services = vec!["pcx.skills.v1.Skills".to_string()];
            serving.descriptors = post_cortex_proto::SKILLS_DESCRIPTOR_SET.to_vec();
            serving.rpcs = vec![wire::Rpc {
                method: "/pcx.skills.v1.Skills/ListSkills".to_string(),
                tool: "outsider_ask".to_string(),
                visibility: Vec::new(),
                effect: post_cortex_proto::module::Effect::Read.into(),
            }];
            Test::new(serving)
        }
    }
}

fn spec(id: &str, uses: &[&str]) -> ProcessSpec {
    use sha2::Digest;
    let command = std::env::current_exe().unwrap();
    let sha256 = sha2::Sha256::digest(std::fs::read(&command).unwrap())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    ProcessSpec {
        id: id.to_string(),
        command,
        args: vec![id.to_string()],
        sha256,
        config: None,
        grants: Vec::new(),
        uses: uses.iter().map(|u| (*u).to_string()).collect(),
        filesystem: Vec::new(),
    }
}

fn scope() -> Scope {
    Scope {
        tenant: String::new(),
        profile: String::new(),
        session_id: None,
        workspace_id: None,
        author: Some("dev".to_string()),
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

/// The member's `ask` for `prompt` as a client calls it (`kind`
/// `TURN_END`: cancelled at its first event): its answer and its counts.
async fn ask_member(host: &Host, prompt: &str, kind: EventKind) -> (String, String) {
    let mut request = tonic::Request::new(wire::DispatchRequest {
        scope: None,
        kind: kind.into(),
        event: Some(Event {
            prompt: prompt.to_string(),
            ..Event::default()
        }),
    });
    scope().to_metadata(request.metadata_mut()).unwrap();
    let outcome = HooksClient::new(host.channel(Origin::Client))
        .dispatch(request)
        .await
        .unwrap()
        .into_inner()
        .outcome
        .unwrap();
    (outcome.decision, outcome.reason)
}

async fn turns_cross_every_process(dir: &Path) {
    let mut modules: Vec<Arc<dyn Module>> = Vec::new();
    for (id, uses) in [
        ("provider", &[][..]),
        ("context", &[][..]),
        ("agent", &["member", "outsider"][..]),
        ("member", &["agent", "outsider"][..]),
        ("outsider", &[][..]),
        ("speaker", &[][..]),
    ] {
        modules.push(Arc::new(
            ProcessModule::launch(&spec(id, uses), dir).await.unwrap(),
        ));
    }
    let host = Host::start(modules, &HashMap::new(), &HashMap::new())
        .await
        .unwrap();

    // The provider's process is listed again once it started: its
    // listing at launch had no settings.
    let models: Vec<String> = host.models().into_iter().map(|m| m.id).collect();
    assert!(models.iter().any(|m| m == STARTED_MODEL), "{models:?}");

    // A member's turn: member → host → agent → host → context and provider.
    let asked = ask_member(&host, "hi", EventKind::TurnStart).await;
    assert_eq!(
        asked,
        (
            "hello [recalled s1] as dev with be brief".to_string(),
            "1 1 7 false".to_string()
        )
    );

    // A client's turn, as `RunTurn` asks the host's agent.
    let (tx, mut rx) = mpsc::channel(16);
    let end = host
        .turns()
        .unwrap()
        .run(&scope(), &TurnRequest::new("s2", "hi"), &tx)
        .await
        .unwrap();
    assert_eq!(end.answer, "hello [recalled s2] as dev with ");
    assert_eq!(end.usage.input, 7);
    assert_eq!(rx.recv().await, Some(TurnEvent::Text("hello".into())));

    // The member's turn lists what the member reaches (not its own tool,
    // which a client's turn of the agent lists): the token crossed from
    // the host to the agent's process and back on its callback socket.
    let listed = ask_member(&host, "tools", EventKind::TurnStart).await;
    assert_eq!(listed.0, "outsider_ask");
    let (tx, _rx) = mpsc::channel(16);
    let own = host
        .turns()
        .unwrap()
        .run(&scope(), &TurnRequest::new("s3", "tools"), &tx)
        .await
        .unwrap();
    assert_eq!(own.answer, "ask,outsider_ask");

    // A cancel reaches the agent, and its end is still read whole.
    let cancelled = ask_member(&host, "wait", EventKind::TurnEnd).await;
    assert_eq!(cancelled.1, "1 1 3 true");

    // A module process that runs turns in its own name serves them on its
    // socket.
    let (tx, _rx) = mpsc::channel(16);
    let spoken = host
        .turns_of("speaker")
        .unwrap()
        .run(&scope(), &TurnRequest::new("s4", "tools"), &tx)
        .await
        .unwrap();
    assert_eq!(spoken.answer, "");
    assert_eq!(spoken.steps, 0);

    // A module that does not use the agent gets no turns.
    let mut request = tonic::Request::new(skills::ListSkillsRequest::default());
    scope().to_metadata(request.metadata_mut()).unwrap();
    let refused = SkillsClient::new(host.channel(Origin::Client))
        .list_skills(request)
        .await
        .unwrap_err();
    assert_eq!(refused.code(), tonic::Code::PermissionDenied, "{refused}");

    host.stop().await;
}

fn main() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    if let Some(socket) = std::env::var_os(SOCKET_ENV) {
        let which = std::env::args().nth(1).unwrap();
        runtime
            .block_on(serve(Arc::new(module(&which)), Path::new(&socket)))
            .unwrap();
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    runtime.block_on(turns_cross_every_process(dir.path()));
    println!("test result: ok. 1 passed (slots across processes)");
}
