// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The models on a module process's callback socket: listed and asked over
//! the socket for a module in the agent's slot, refused to any other; and
//! what stops a module, which it reports there.

use super::*;

use std::collections::HashMap;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use parking_lot::Mutex;
use pcx_module_api::model::{
    Block, Modality, ModelEvent, ModelFailure, ModelFailureKind, ModelInfo, ModelKind,
    ModelProvider, ModelRequest, ModelRun, RemoteModels, StopReason, ToolChoice, Usage,
};
use pcx_module_api::{
    API_VERSION, Event, HostContext, Manifest, Module, ModuleError, ModuleErrorKind, ModuleKind,
    Outcome, Resource,
};
use tokio::sync::mpsc;

use crate::Host;

/// A module's calls to the modules' gRPC services, which these tests make
/// none of.
fn no_services() -> GrpcChannel {
    grpc::refusing(Status::unimplemented("no module serves gRPC here"))
}

/// Serves `glm`, answering with its name.
struct Glm;

#[async_trait]
impl ModelProvider for Glm {
    fn models(&self) -> Vec<ModelInfo> {
        vec![ModelInfo {
            id: "glm".into(),
            context_tokens: 1000,
            output_tokens: 100,
            kind: ModelKind::Chat as i32,
            input: vec![Modality::Text as i32],
            output: vec![Modality::Text as i32],
            ..ModelInfo::default()
        }]
    }

    async fn generate(&self, _: &Scope, request: ModelRequest) -> Result<ModelRun, ModelFailure> {
        let (tx, rx) = mpsc::channel(1);
        let done = ModelEvent::Done {
            blocks: vec![Block::Text {
                text: request.model,
            }],
            usage: Usage::default(),
            stop: StopReason::EndTurn,
        };
        tx.send(Ok(done))
            .await
            .map_err(|_| ModelFailure::new(ModelFailureKind::Unavailable, "dropped"))?;
        Ok(ModelRun::new(rx))
    }
}

/// A module of `kind` that keeps its handle; a provider serves `glm`.
struct Held {
    manifest: Manifest,
    host: Mutex<Option<Arc<dyn ModuleHost>>>,
}

fn held(id: &str, kind: ModuleKind) -> Arc<Held> {
    Arc::new(Held {
        manifest: Manifest {
            id: id.into(),
            version: "0.1.0".into(),
            api: Some(API_VERSION),
            kind: kind.into(),
            ..Manifest::default()
        },
        host: Mutex::new(None),
    })
}

#[async_trait]
impl Module for Held {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    async fn start(&self, ctx: HostContext) -> Result<(), ModuleError> {
        *self.host.lock() = Some(ctx.host);
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

    fn model_provider(&self) -> Option<Arc<dyn ModelProvider>> {
        (self.manifest.kind == i32::from(ModuleKind::ModelProvider))
            .then(|| Arc::new(Glm) as Arc<dyn ModelProvider>)
    }
}

/// A channel to the Unix socket `socket`.
fn channel(socket: std::path::PathBuf) -> tonic::transport::Channel {
    tonic::transport::Endpoint::from_static("http://host.sock").connect_with_connector_lazy(
        tower::service_fn(move |_: tonic::transport::Uri| {
            let socket = socket.clone();
            async move {
                tokio::net::UnixStream::connect(socket)
                    .await
                    .map(hyper_util::rt::TokioIo::new)
            }
        }),
    )
}

fn scope() -> Scope {
    Scope {
        tenant: String::new(),
        profile: String::new(),
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
        deadline: Instant::now() + Duration::from_secs(5),
        cancel: CancellationToken::new(),
    }
}

fn request(model: &str) -> ModelRequest {
    ModelRequest {
        model: model.into(),
        system: String::new(),
        messages: Vec::new(),
        tools: Vec::new(),
        max_tokens: 10,
        tool_choice: ToolChoice::Auto,
    }
}

#[tokio::test]
async fn a_module_reports_what_stops_it_on_its_socket_and_its_health_and_log_say_so() {
    use post_cortex_proto::module::host_client::HostClient;

    let tool = held("tool", ModuleKind::Tool);
    let host = Host::start(vec![tool.clone()], &HashMap::new(), &HashMap::new())
        .await
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("tool.sock");
    let handle = tool.host.lock().clone().unwrap();
    let served = Callback::serve(handle, no_services(), &socket, "tool").unwrap();
    let mut client = HostClient::new(channel(socket));
    let mut report = async |problem: &str| {
        client
            .report(wire::ReportRequest {
                problem: problem.into(),
            })
            .await
            .unwrap()
            .into_inner()
    };
    assert_eq!(host.health()[0].problem, "");

    // Said twice (it looks again each minute), written once.
    let lacks = "no forum the bot posts to has a tag answered";
    assert!(report(lacks).await.error.is_none());
    assert!(report(lacks).await.error.is_none());
    assert_eq!(host.health()[0].problem, lacks);
    assert_eq!(host.module("tool").unwrap().health.problem, lacks);
    let said = |what: &str| {
        host.module_log("tool")
            .unwrap()
            .iter()
            .filter(|line| line.text.contains(what))
            .count()
    };
    assert_eq!(said(&format!("it cannot work: {lacks}")), 1);

    // Nothing stops it any more.
    assert!(report("").await.error.is_none());
    assert_eq!(host.health()[0].problem, "");
    assert_eq!(said("it works again"), 1);
    served.stop("tool").await;
}

#[tokio::test]
async fn the_agents_models_are_served_on_its_socket_and_refused_to_others() {
    let (agent, tool) = (
        held("agent", ModuleKind::Agent),
        held("tool", ModuleKind::Tool),
    );
    let modules: Vec<Arc<dyn Module>> = vec![
        held("zai", ModuleKind::ModelProvider),
        agent.clone(),
        tool.clone(),
    ];
    let _host = Host::start(modules, &HashMap::new(), &HashMap::new())
        .await
        .unwrap();
    let dir = tempfile::tempdir().unwrap();

    let socket = dir.path().join("agent.sock");
    let handle = agent.host.lock().clone().unwrap();
    let served = Callback::serve(handle, no_services(), &socket, "agent").unwrap();
    let models = RemoteModels::connect(channel(socket)).await.unwrap();
    assert_eq!(models.models()[0].id, "glm");
    let (blocks, _, stop) = models
        .generate(&scope(), request("glm"))
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert_eq!(
        (blocks, stop),
        (
            vec![Block::Text { text: "glm".into() }],
            StopReason::EndTurn
        )
    );
    let unknown = models
        .generate(&scope(), request("none"))
        .await
        .unwrap()
        .collect()
        .await
        .unwrap_err();
    assert_eq!(unknown.kind, ModelFailureKind::Invalid);
    served.stop("agent").await;

    let socket = dir.path().join("tool.sock");
    let handle = tool.host.lock().clone().unwrap();
    let served = Callback::serve(handle, no_services(), &socket, "tool").unwrap();
    let denied = RemoteModels::connect(channel(socket)).await.err().unwrap();
    assert_eq!(denied.kind, ModuleErrorKind::Denied);
    served.stop("tool").await;
}
