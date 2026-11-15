// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Models through the host handle: offered to the agent's slot only, each
//! named by its provider (as its module names it, else the module's id), a
//! request routed to the provider that lists its model, once its listing is
//! in, and refused for a model none lists; a model id two providers serve,
//! and a model out of shape, are refused at load.

use super::*;

use parking_lot::Mutex as SyncMutex;
use pcx_module_api::ModuleHost;
use pcx_module_api::model::{
    Block, Modality, ModelEvent, ModelFailure, ModelFailureKind, ModelInfo, ModelKind,
    ModelProvider, ModelRequest, ModelRun, StopReason, ToolChoice, Usage,
};
use tokio::sync::mpsc;

/// Serves `models`, answering each with its own name; a model whose id
/// begins `bad-` has no kind, one whose id begins `late-` is listed only
/// once its listing is in; the models of `other` name their provider.
struct Provider {
    manifest: Manifest,
    models: Vec<&'static str>,
}

impl Provider {
    fn infos(&self, late: bool) -> Vec<ModelInfo> {
        self.models
            .iter()
            .filter(|id| late || !id.starts_with("late-"))
            .map(|id| ModelInfo {
                id: (*id).to_string(),
                context_tokens: 1000,
                output_tokens: 100,
                kind: if id.starts_with("bad-") {
                    ModelKind::Unspecified as i32
                } else {
                    ModelKind::Chat as i32
                },
                input: vec![Modality::Text as i32],
                output: vec![Modality::Text as i32],
                provider: if self.manifest.id == "other" {
                    "its api".to_string()
                } else {
                    String::new()
                },
                ..ModelInfo::default()
            })
            .collect()
    }
}

#[async_trait]
impl ModelProvider for Provider {
    fn models(&self) -> Vec<ModelInfo> {
        self.infos(false)
    }

    async fn listed(&self) -> Result<Vec<ModelInfo>, ModuleError> {
        Ok(self.infos(true))
    }

    async fn generate(&self, _: &Scope, request: ModelRequest) -> Result<ModelRun, ModelFailure> {
        let (tx, rx) = mpsc::channel(1);
        let blocks = vec![Block::Text {
            text: format!("{} by {}", request.model, self.manifest.id),
        }];
        let done = ModelEvent::Done {
            blocks,
            usage: Usage::default(),
            stop: StopReason::EndTurn,
        };
        tx.send(Ok(done))
            .await
            .map_err(|_| ModelFailure::new(ModelFailureKind::Unavailable, "dropped"))?;
        Ok(ModelRun::new(rx))
    }
}

/// A module of `kind` that keeps its host handle.
struct Holder {
    manifest: Manifest,
    host: SyncMutex<Option<Arc<dyn ModuleHost>>>,
    provider: Option<Arc<Provider>>,
}

fn module(id: &str, kind: ModuleKind, models: Vec<&'static str>) -> Arc<Holder> {
    let manifest = Manifest {
        id: id.to_string(),
        version: "0.1.0".to_string(),
        api: Some(API_VERSION),
        kind: kind.into(),
        ..Manifest::default()
    };
    let provider = (kind == ModuleKind::ModelProvider).then(|| {
        Arc::new(Provider {
            manifest: manifest.clone(),
            models,
        })
    });
    Arc::new(Holder {
        manifest,
        host: SyncMutex::new(None),
        provider,
    })
}

#[async_trait]
impl Module for Holder {
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
        self.provider.clone().map(|p| p as Arc<dyn ModelProvider>)
    }
}

fn request(model: &str) -> ModelRequest {
    ModelRequest {
        model: model.to_string(),
        system: String::new(),
        messages: Vec::new(),
        tools: Vec::new(),
        max_tokens: 10,
        tool_choice: ToolChoice::Auto,
    }
}

#[tokio::test]
async fn the_agent_reaches_each_model_through_its_provider() {
    let agent = module("agent", ModuleKind::Agent, Vec::new());
    let tool = module("tool", ModuleKind::Tool, Vec::new());
    let modules: Vec<Arc<dyn Module>> = vec![
        module("zai", ModuleKind::ModelProvider, vec!["glm"]),
        module(
            "other",
            ModuleKind::ModelProvider,
            vec!["big", "small", "late-one"],
        ),
        agent.clone(),
        tool.clone(),
    ];
    let host = Host::start(modules, &HashMap::new(), &HashMap::new())
        .await
        .unwrap();
    let scope = scope(Duration::from_secs(1));

    let handle = agent.host.lock().clone().unwrap();
    let models = handle.model().await.unwrap();
    let listed: Vec<(String, String)> = models
        .models()
        .into_iter()
        .map(|m| (m.id, m.provider))
        .collect();
    let expected = [("glm", "zai"), ("big", "its api"), ("small", "its api")]
        .map(|(id, by)| (id.to_string(), by.to_string()));
    assert_eq!(listed, expected);
    assert_eq!(host.models(), models.models());
    // A model not listed yet is served once its provider's listing is in.
    for (model, by) in [
        ("glm", "zai"),
        ("big", "other"),
        ("small", "other"),
        ("late-one", "other"),
    ] {
        let (blocks, _, _) = models
            .generate(&scope, request(model))
            .await
            .unwrap()
            .collect()
            .await
            .unwrap();
        assert_eq!(
            blocks,
            [Block::Text {
                text: format!("{model} by {by}")
            }]
        );
    }
    let unknown = models
        .generate(&scope, request("none"))
        .await
        .err()
        .unwrap();
    assert_eq!(unknown.kind, ModelFailureKind::Invalid);

    // A tool module is not offered models.
    let other = tool.host.lock().clone().unwrap();
    let denied = other.model().await.err().unwrap();
    assert_eq!(denied.kind, ModuleErrorKind::Denied);
}

#[tokio::test]
async fn a_model_two_providers_serve_a_model_out_of_shape_or_a_provider_without_models_is_refused()
{
    let twice: Vec<Arc<dyn Module>> = vec![
        module("a", ModuleKind::ModelProvider, vec!["glm"]),
        module("b", ModuleKind::ModelProvider, vec!["glm"]),
    ];
    let refused = Host::start(twice, &HashMap::new(), &HashMap::new()).await;
    assert!(matches!(refused, Err(HostError::DuplicateModel { .. })));

    let shapeless: Vec<Arc<dyn Module>> =
        vec![module("d", ModuleKind::ModelProvider, vec!["bad-glm"])];
    let refused = Host::start(shapeless, &HashMap::new(), &HashMap::new()).await;
    assert!(matches!(refused, Err(HostError::InvalidModel { .. })));

    let mut empty = module("c", ModuleKind::Tool, Vec::new());
    Arc::get_mut(&mut empty).unwrap().manifest.kind = ModuleKind::ModelProvider.into();
    let refused = Host::start(
        vec![empty as Arc<dyn Module>],
        &HashMap::new(),
        &HashMap::new(),
    )
    .await;
    assert!(matches!(refused, Err(HostError::InvalidManifest { .. })));
}

#[tokio::test]
async fn the_agent_sees_the_tools_it_uses_and_sends_its_events() {
    let mut agent = module("agent", ModuleKind::Agent, Vec::new());
    Arc::get_mut(&mut agent).unwrap().manifest.uses = vec!["echo".to_string()];
    let tool = module("tool", ModuleKind::Tool, Vec::new());
    let modules: Vec<Arc<dyn Module>> = vec![
        Arc::new(fake(
            "echo",
            ModuleKind::Tool,
            Act::Block,
            FailurePolicy::FailClosed,
        )),
        Arc::new(fake(
            "other",
            ModuleKind::Tool,
            Act::Answer,
            FailurePolicy::FailOpen,
        )),
        agent.clone(),
        tool.clone(),
    ];
    let _host = Host::start(modules, &HashMap::new(), &HashMap::new())
        .await
        .unwrap();
    let handle = agent.host.lock().clone().unwrap();
    let scope = scope(Duration::from_secs(1));

    // Its turn's event reaches every subscriber; a fail-closed block holds.
    let outcome = handle
        .dispatch(&scope, EventKind::TurnStart, &prompt())
        .await
        .unwrap();
    assert_eq!(outcome.decision, "block");

    // Another module sends no events.
    let other = tool.host.lock().clone().unwrap();
    let denied = other
        .dispatch(&scope, EventKind::TurnStart, &prompt())
        .await
        .unwrap_err();
    assert_eq!(denied.kind, ModuleErrorKind::Denied);
}
