// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The out-of-process runner end to end. This binary is both the test and
//! the module: started with `PCX_MODULE_SOCKET` it serves an echo module on
//! that socket (`pcx_module_api::serve`); without it, it launches itself as
//! that module through the host and checks launch, start with the
//! configuration, its RPC called by path (in its own process group),
//! events, views (an HTML one read over the protocol), prompts, its calls
//! to the modules it uses through its callback socket, stop and the
//! refusals.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use parking_lot::Mutex;
use pcx_host::{Grant, Host, HostError, Origin, ProcessModule, ProcessSpec};
use pcx_module_api::grpc::{self, GrpcChannel};
use pcx_module_api::view::HTML_MIME;
use pcx_module_api::{
    API_VERSION, Event, EventKind, HostContext, LogSource, Manifest, Module, ModuleError,
    ModuleErrorKind, ModuleKind, Outcome, Resource, SOCKET_ENV, Scope, serve,
};
use post_cortex_proto::module as wire;
use post_cortex_proto::module::hooks_client::HooksClient;
use post_cortex_proto::skills;
use post_cortex_proto::skills::skills_server::{Skills, SkillsServer};
use serde_json::json;
use tokio_util::sync::CancellationToken;

mod echo;
use echo::{CONFIG, DISPATCH, Echo, LIST_SKILLS, process_group};

/// Who called the helper's RPC, and how deep.
type Calls = Arc<Mutex<Vec<(String, u32)>>>;

/// The module echo uses, in the host's process: its RPC keeps who called.
struct Helper {
    manifest: Manifest,
    calls: Calls,
}

impl Helper {
    fn new(calls: Calls) -> Self {
        Self {
            manifest: Manifest {
                id: "helper".to_string(),
                version: "1.0.0".to_string(),
                api: Some(API_VERSION),
                kind: ModuleKind::Tool.into(),
                services: vec!["pcx.skills.v1.Skills".to_string()],
                descriptors: post_cortex_proto::SKILLS_DESCRIPTOR_SET.to_vec(),
                rpcs: vec![wire::Rpc {
                    method: LIST_SKILLS.to_string(),
                    tool: "whoami".to_string(),
                    visibility: Vec::new(),
                    effect: post_cortex_proto::module::Effect::Read.into(),
                }],
                ..Manifest::default()
            },
            calls,
        }
    }
}

/// The helper's RPC, which keeps who called it.
struct Whoami(Calls);

#[tonic::async_trait]
impl Skills for Whoami {
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
        self.0
            .lock()
            .push((scope.caller_module.unwrap_or_default(), scope.call_depth));
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
impl Module for Helper {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    async fn start(&self, _ctx: HostContext) -> Result<(), ModuleError> {
        Ok(())
    }

    async fn on_event(&self, _: &Scope, _: &Event) -> Result<Outcome, ModuleError> {
        Ok(Outcome::default())
    }

    async fn resource(&self, uri: &str) -> Result<Resource, ModuleError> {
        Err(ModuleError::new(ModuleErrorKind::NotFound, uri))
    }

    async fn stop(&self) -> Result<(), ModuleError> {
        Ok(())
    }

    fn services(&self) -> Option<GrpcChannel> {
        let whoami = Whoami(Arc::clone(&self.calls));
        Some(grpc::routes(tonic::service::Routes::new(
            SkillsServer::new(whoami),
        )))
    }
}

/// Echo's tool `prompt` as a client calls it, in [`scope`]: its answer.
async fn echo(host: &Host, prompt: &str) -> Result<String, tonic::Status> {
    let mut request = tonic::Request::new(wire::DispatchRequest {
        scope: None,
        kind: EventKind::PreTool.into(),
        event: Some(Event {
            prompt: prompt.to_string(),
            ..Event::default()
        }),
    });
    scope().to_metadata(request.metadata_mut()).unwrap();
    let answer = HooksClient::new(host.channel(Origin::Client))
        .dispatch(request)
        .await?
        .into_inner();
    Ok(answer.outcome.unwrap().decision)
}

fn spec(id: &str) -> ProcessSpec {
    let command = std::env::current_exe().unwrap();
    let sha256 = sha256_hex(&std::fs::read(&command).unwrap());
    ProcessSpec {
        id: id.to_string(),
        command,
        args: Vec::new(),
        sha256,
        config: Some(json!({"greeting": "hello"})),
        grants: vec![Grant::ReadConversation, Grant::UiHtml],
        uses: vec!["helper".to_string()],
        filesystem: Vec::new(),
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
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

async fn launches_calls_and_stops(dir: &Path) {
    let spec = spec("echo");
    let module = ProcessModule::launch(&spec, dir).await.unwrap();
    let config = HashMap::from([("echo".to_string(), spec.config.clone().unwrap())]);
    let host = Host::start(vec![Arc::new(module)], &config, &HashMap::new())
        .await
        .unwrap();

    // Its socket has explicit rights, its user's and its directory's group's
    // (docs/modules/dynamic.md, the host's sockets), whatever the umask.
    let socket = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path().join("m.sock"))
        .find(|socket| socket.exists())
        .unwrap();
    assert_eq!(
        std::os::unix::fs::MetadataExt::mode(&std::fs::metadata(&socket).unwrap()) & 0o777,
        0o660
    );
    // Its RPC, called by path on its socket in the caller's scope.
    assert_eq!(echo(&host, "echo").await.unwrap(), "for claude-code");
    // The module leads its own process group, apart from the host's.
    let group = echo(&host, "group").await.unwrap();
    let (pid, pgid) = group.split_once(' ').unwrap();
    assert_eq!(pid, pgid, "the module leads its group");
    assert_ne!(
        pgid.parse::<u32>().unwrap(),
        process_group(std::process::id()),
        "apart from the host's group"
    );
    // What it writes on stderr is in its log (W25).
    assert_eq!(echo(&host, "say").await.unwrap(), "said");
    let said = async {
        loop {
            let log = host.module_log("echo").unwrap();
            if log.iter().any(|l| l.text == "said hello") {
                return log;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    let log = tokio::time::timeout(Duration::from_secs(5), said)
        .await
        .expect("the module's stderr line reached its log");
    assert!(
        log.iter()
            .any(|l| l.text == "said hello" && l.source == LogSource::Process)
    );
    let unknown = echo(&host, "nope").await.unwrap_err();
    assert_eq!(unknown.code(), tonic::Code::NotFound);
    // Its RPC is its tool, for agents and its views.
    let names: Vec<String> = host.tools().into_iter().map(|t| t.name).collect();
    assert_eq!(names, ["echo"]);
    assert_eq!(host.rpc_path("echo").as_deref(), Some(DISPATCH));

    // The HTML view, read from the process.
    let card = host.view(&scope(), "echo", "card").await.unwrap();
    let tools: Vec<&str> = card.rpcs.iter().map(|r| r.tool.as_str()).collect();
    assert_eq!(tools, ["echo"]);
    assert_eq!(card.view.id, "card");
    let resource = card.resource.unwrap();
    assert_eq!(
        (resource.mime_type.as_str(), resource.text.as_str()),
        (HTML_MIME, "<p>hello</p>")
    );

    // Its prompts, listed and filled over the protocol in the caller's scope.
    let prompts: Vec<String> = host
        .prompts(&scope())
        .await
        .into_iter()
        .map(|p| p.name)
        .collect();
    assert_eq!(prompts, ["greeting"]);
    let arguments = HashMap::from([("name".to_string(), "Ana".to_string())]);
    let filled = host.prompt(&scope(), "greeting", &arguments).await.unwrap();
    assert_eq!(filled.messages[0].text, "hello Ana, from claude-code");
    let missing = host.prompt(&scope(), "nope", &arguments).await.unwrap_err();
    assert_eq!(missing.kind, ModuleErrorKind::NotFound);

    let event: Event = serde_json::from_value(json!({
        "hook_event_name": "PreToolUse", "tool_name": "Edit"
    }))
    .unwrap();
    for _ in 0..3 {
        let dispatched = host.dispatch(&scope(), EventKind::PreTool, &event).await;
        assert!(dispatched.failures.is_empty(), "{:?}", dispatched.failures);
        let added = dispatched.outcome.hook_specific_output.unwrap();
        assert_eq!(added.additional_context, "hello Edit");
    }

    host.stop().await;
    let prefix = format!("pcx-{}-", std::process::id());
    let left: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(&prefix) && name.ends_with("-echo"))
        .collect();
    assert!(left.is_empty(), "{left:?}");
}

async fn calls_the_modules_it_uses(dir: &Path) {
    let spec = spec("echo");
    let module = ProcessModule::launch(&spec, dir).await.unwrap();
    let config = HashMap::from([("echo".to_string(), spec.config.clone().unwrap())]);
    let calls = Calls::default();
    let host = Host::start(
        vec![Arc::new(Helper::new(Arc::clone(&calls))), Arc::new(module)],
        &config,
        &HashMap::new(),
    )
    .await
    .unwrap();
    // Over its callback socket: the running modules, and a call of the
    // module it uses, which the host names it the caller of, one deep.
    assert_eq!(echo(&host, "modules").await.unwrap(), "helper,echo");
    assert_eq!(echo(&host, "relay").await.unwrap(), "relayed");
    assert_eq!(*calls.lock(), [("echo".to_string(), 1)]);
    host.stop().await;
}

async fn describes_without_starting(dir: &Path) {
    let manifest = ProcessModule::describe(&spec("echo"), dir).await.unwrap();
    assert_eq!(manifest.id, "echo");
    assert_eq!(manifest.config_type, CONFIG);
    let prefix = format!("pcx-{}-", std::process::id());
    let left: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(&prefix))
        .collect();
    assert!(left.is_empty(), "the described module left {left:?}");
    let mut ungranted = spec("echo");
    ungranted.grants = Vec::new();
    assert!(matches!(
        ProcessModule::describe(&ungranted, dir).await,
        Err(HostError::NotGranted { .. })
    ));
}

async fn refuses_a_configuration_outside_its_schema(dir: &Path) {
    let module = ProcessModule::launch(&spec("echo"), dir).await.unwrap();
    let config = HashMap::from([("echo".to_string(), json!({}))]);
    let refused = Host::start(vec![Arc::new(module)], &config, &HashMap::new()).await;
    match refused {
        Err(HostError::InvalidConfig { module, error }) => {
            assert_eq!(module, "echo");
            assert_eq!(error.field, "greeting");
        }
        other => panic!("expected InvalidConfig, got {:?}", other.err()),
    }
    let prefix = format!("pcx-{}-", std::process::id());
    let left = std::fs::read_dir(dir)
        .unwrap()
        .filter(|e| {
            let name = e.as_ref().unwrap().file_name();
            name.to_string_lossy().starts_with(&prefix)
        })
        .count();
    assert_eq!(left, 0, "a refused module leaves no socket");
}

async fn refuses_an_unapproved_executable(dir: &Path) {
    let mut spec = spec("echo");
    spec.sha256 = "00".repeat(32);
    assert!(matches!(
        ProcessModule::launch(&spec, dir).await,
        Err(HostError::ExecutableNotApproved { .. })
    ));
}

async fn refuses_a_process_that_asks_for_more_than_granted(dir: &Path) {
    let mut spec = spec("echo");
    spec.grants = Vec::new();
    assert!(matches!(
        ProcessModule::launch(&spec, dir).await,
        Err(HostError::NotGranted { .. })
    ));
}

async fn refuses_a_process_that_names_another_id(dir: &Path) {
    assert!(matches!(
        ProcessModule::launch(&spec("other"), dir).await,
        Err(HostError::InvalidManifest { .. })
    ));
}

fn main() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    if let Some(socket) = std::env::var_os(SOCKET_ENV) {
        runtime
            .block_on(serve(Arc::new(Echo::new()), Path::new(&socket)))
            .unwrap();
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    runtime.block_on(async {
        launches_calls_and_stops(dir.path()).await;
        calls_the_modules_it_uses(dir.path()).await;
        describes_without_starting(dir.path()).await;
        refuses_a_configuration_outside_its_schema(dir.path()).await;
        refuses_an_unapproved_executable(dir.path()).await;
        refuses_a_process_that_asks_for_more_than_granted(dir.path()).await;
        refuses_a_process_that_names_another_id(dir.path()).await;
    });
    println!("test result: ok. 7 passed (process module)");
}
