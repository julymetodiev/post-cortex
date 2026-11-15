// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The bindings end to end (docs/modules/dynamic.md): the greet example and
//! the relay fixture, in Python (free-threaded CPython), and in TypeScript
//! on Deno and on Node, each run by the real host as a process module.
//! They are called by path through the host, call a Rust module in the
//! host's process through theirs, fail with a kind, and outlive a caller
//! that gave up.
//!
//! The runtimes and the built bindings come from `scripts/test-bindings.sh`,
//! which names them in `PCX_BINDINGS_PYTHON`, `PCX_BINDINGS_DENO` and
//! `PCX_BINDINGS_NODE`; without it the tests are ignored.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use pcx_bindings_tests::RELAY_DESCRIPTORS;
use pcx_bindings_tests::greet::GreetRequest;
use pcx_bindings_tests::greet::greeter_client::GreeterClient;
use pcx_bindings_tests::relay::Text;
use pcx_bindings_tests::relay::helper_server::{Helper as HelperRpc, HelperServer};
use pcx_bindings_tests::relay::relay_client::RelayClient;
use pcx_host::{Host, Origin, ProcessModule, ProcessSpec};
use pcx_module_api::grpc::{self, GrpcChannel};
use pcx_module_api::{
    API_VERSION, Effect, Event, HostContext, Manifest, Module, ModuleError, ModuleErrorKind,
    ModuleKind, Outcome, Resource, Rpc, Scope,
};
use tokio_util::sync::CancellationToken;

mod confined;

/// The repository's root.
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A runtime the bindings' script named.
fn runtime(env: &str) -> PathBuf {
    std::env::var_os(env)
        .map(PathBuf::from)
        .unwrap_or_else(|| panic!("{env} is not set: run scripts/test-bindings.sh"))
}

/// How a module in one language is started.
#[derive(Clone, Copy)]
enum Language {
    Python,
    Deno,
    Node,
}

impl Language {
    /// The process of `script` (its path from the root, without the
    /// extension) as the host runs it.
    fn spec(self, id: &str, script: &str, uses: &[&str]) -> ProcessSpec {
        let (command, args) = match self {
            Self::Python => (runtime("PCX_BINDINGS_PYTHON"), vec![format!("{script}.py")]),
            Self::Deno => (
                runtime("PCX_BINDINGS_DENO"),
                ["run", "--no-prompt", "--allow-all"]
                    .map(String::from)
                    .into_iter()
                    .chain([format!("{script}.ts")])
                    .collect(),
            ),
            Self::Node => (runtime("PCX_BINDINGS_NODE"), vec![format!("{script}.ts")]),
        };
        let args = args
            .into_iter()
            .map(|a| {
                if a.ends_with(".py") || a.ends_with(".ts") {
                    root().join(a).display().to_string()
                } else {
                    a
                }
            })
            .collect();
        ProcessSpec {
            id: id.to_string(),
            sha256: sha256_hex(&command),
            command,
            args,
            config: None,
            grants: Vec::new(),
            uses: uses.iter().map(|u| (*u).to_string()).collect(),
            filesystem: Vec::new(),
        }
    }
}

fn sha256_hex(path: &Path) -> String {
    use sha2::Digest;
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    sha2::Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A call's scope, of the person `tester`.
fn scope(deadline: Duration) -> Scope {
    Scope {
        tenant: String::new(),
        profile: String::new(),
        session_id: None,
        workspace_id: None,
        author: Some("tester".to_string()),
        caller_module: None,
        call_depth: 0,
        turn: None,
        reach: None,
        role: None,
        read_only: false,
        person: None,
        deadline: Instant::now() + deadline,
        cancel: CancellationToken::new(),
    }
}

/// `message` as a request of the person `tester`.
fn request<T>(message: T, deadline: Duration) -> tonic::Request<T> {
    let mut request = tonic::Request::new(message);
    scope(deadline).to_metadata(request.metadata_mut()).unwrap();
    request.set_timeout(deadline);
    request
}

/// The Rust module the relay uses, in the host's process.
struct Helper {
    manifest: Manifest,
}

impl Helper {
    fn new() -> Self {
        Self {
            manifest: Manifest {
                id: "helper".to_string(),
                version: "1.0.0".to_string(),
                api: Some(API_VERSION),
                kind: ModuleKind::Tool.into(),
                services: vec!["test.relay.v1.Helper".to_string()],
                descriptors: RELAY_DESCRIPTORS.to_vec(),
                rpcs: vec![Rpc {
                    method: "/test.relay.v1.Helper/Upper".to_string(),
                    tool: String::new(),
                    visibility: Vec::new(),
                    effect: Effect::Read.into(),
                }],
                ..Manifest::default()
            },
        }
    }
}

/// Upper: the text in capitals, from the module that called.
struct Upper;

#[tonic::async_trait]
impl HelperRpc for Upper {
    async fn upper(
        &self,
        request: tonic::Request<Text>,
    ) -> Result<tonic::Response<Text>, tonic::Status> {
        let caller = Scope::of(&request)?.caller_module.unwrap_or_default();
        let text = request.into_inner().text.to_uppercase();
        Ok(tonic::Response::new(Text {
            text: format!("{text} from {caller}"),
            wait_ms: 0,
        }))
    }
}

#[async_trait]
impl Module for Helper {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    async fn start(&self, _: HostContext) -> Result<(), ModuleError> {
        Ok(())
    }

    async fn on_event(&self, _: &Scope, _: &Event) -> Result<Outcome, ModuleError> {
        Ok(Outcome::default())
    }

    async fn resource(&self, uri: &str) -> Result<Resource, ModuleError> {
        Err(ModuleError::new(ModuleErrorKind::NotFound, uri.to_string()))
    }

    async fn stop(&self) -> Result<(), ModuleError> {
        Ok(())
    }

    fn services(&self) -> Option<GrpcChannel> {
        Some(grpc::routes(tonic::service::Routes::new(
            HelperServer::new(Upper),
        )))
    }
}

/// The host running `spec`, after `inprocess`.
async fn host(spec: &ProcessSpec, dir: &Path, inprocess: Vec<Arc<dyn Module>>) -> Host {
    let module = ProcessModule::launch(spec, dir)
        .await
        .unwrap_or_else(|e| panic!("launch {}: {e}", spec.id));
    let mut modules = inprocess;
    modules.push(Arc::new(module));
    Host::start(modules, &HashMap::new(), &HashMap::new())
        .await
        .unwrap()
}

/// The greet example: its tool in the host's list, its RPC by path.
async fn greets(language: Language, script: &str, id: &str) {
    let dir = tempfile::tempdir().unwrap();
    let host = host(&language.spec(id, script, &[]), dir.path(), Vec::new()).await;
    let greet = host
        .tools()
        .into_iter()
        .find(|t| t.name == "greet")
        .expect("the greet tool is listed");
    assert_eq!(greet.effect, Effect::Read);
    let answer = GreeterClient::new(host.channel(Origin::Client))
        .greet(request(
            GreetRequest {
                name: "Ada".to_string(),
            },
            Duration::from_secs(10),
        ))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(answer.text, "Hello, Ada");
    host.stop().await;
}

/// The relay fixture: its call through its host, its typed failure, a
/// caller that gives up and a call after it.
async fn relays(language: Language) {
    let dir = tempfile::tempdir().unwrap();
    let spec = language.spec("relay", "bindings/tests/fixtures/relay_module", &["helper"]);
    let host = host(&spec, dir.path(), vec![Arc::new(Helper::new())]).await;
    let relay = || RelayClient::new(host.channel(Origin::Client));
    let text = |text: &str, wait_ms: u32| Text {
        text: text.to_string(),
        wait_ms,
    };

    // Through its host: the helper sees the relay as its caller.
    let answer = relay()
        .upper(request(text("abc", 0), Duration::from_secs(10)))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(answer.text, "ABC from relay for tester");

    // A failure of a kind arrives as that kind.
    let refused = relay()
        .upper(request(text("", 0), Duration::from_secs(10)))
        .await
        .unwrap_err();
    assert_eq!(refused.code(), tonic::Code::InvalidArgument, "{refused:?}");
    assert!(
        refused.message().contains("no text to relay"),
        "{refused:?}"
    );

    // A caller that gives up gets its deadline; the module goes on.
    let started = Instant::now();
    let late = relay()
        .wait(request(text("", 1500), Duration::from_millis(300)))
        .await
        .unwrap_err();
    assert_eq!(late.code(), tonic::Code::DeadlineExceeded, "{late:?}");
    assert!(started.elapsed() < Duration::from_millis(1200));
    let waited = relay()
        .wait(request(text("", 10), Duration::from_secs(10)))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(waited.text, "waited");

    // Calls at once run at once (the module's concurrency).
    let started = Instant::now();
    let calls = (0..4).map(|_| {
        let mut client = relay();
        async move {
            client
                .wait(request(text("", 400), Duration::from_secs(10)))
                .await
        }
    });
    for answer in futures_join(calls).await {
        assert_eq!(answer.unwrap().into_inner().text, "waited");
    }
    assert!(
        started.elapsed() < Duration::from_millis(1200),
        "{:?}",
        started.elapsed()
    );
    host.stop().await;
}

/// Every future of `futures`, awaited together.
async fn futures_join<F: std::future::Future + Send + 'static>(
    futures: impl Iterator<Item = F>,
) -> Vec<F::Output>
where
    F::Output: Send + 'static,
{
    let tasks: Vec<_> = futures.map(tokio::spawn).collect();
    let mut outputs = Vec::new();
    for task in tasks {
        outputs.push(task.await.unwrap());
    }
    outputs
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the built bindings: scripts/test-bindings.sh"]
async fn python_greets() {
    greets(Language::Python, "examples/greet/python/module", "greet-py").await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the built bindings: scripts/test-bindings.sh"]
async fn python_relays() {
    relays(Language::Python).await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the built bindings: scripts/test-bindings.sh"]
async fn deno_greets() {
    greets(Language::Deno, "examples/greet/ts/module", "greet-ts").await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the built bindings: scripts/test-bindings.sh"]
async fn deno_relays() {
    relays(Language::Deno).await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the built bindings: scripts/test-bindings.sh"]
async fn node_greets() {
    greets(Language::Node, "examples/greet/ts/module", "greet-ts").await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the built bindings: scripts/test-bindings.sh"]
async fn node_relays() {
    relays(Language::Node).await;
}
