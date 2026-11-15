// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use std::time::{Duration, Instant};

use parking_lot::Mutex;
use pcx_module_api::ModuleErrorKind;
use pcx_module_api::model::{Block, ModelEvent, ModelKind, StopReason, ToolChoice, Usage};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::*;

/// A provider serving `now` at once and `later` once its listing is in.
struct Fake {
    now: Vec<&'static str>,
    later: Vec<&'static str>,
    refuses: bool,
    started: Mutex<Option<(String, String)>>,
}

fn fake(now: &[&'static str], later: &[&'static str]) -> Arc<Fake> {
    Arc::new(Fake {
        now: now.to_vec(),
        later: later.to_vec(),
        refuses: false,
        started: Mutex::new(None),
    })
}

fn info(id: &str) -> ModelInfo {
    ModelInfo {
        id: id.to_string(),
        // What a provider says is replaced by its section's name.
        provider: "itself".to_string(),
        kind: ModelKind::Chat.into(),
        ..ModelInfo::default()
    }
}

#[async_trait]
impl ModelProvider for Fake {
    fn models(&self) -> Vec<ModelInfo> {
        self.now.iter().map(|id| info(id)).collect()
    }

    async fn listed(&self) -> Result<Vec<ModelInfo>, ModuleError> {
        Ok(self
            .now
            .iter()
            .chain(&self.later)
            .map(|id| info(id))
            .collect())
    }

    async fn generate(&self, _: &Scope, request: ModelRequest) -> Result<ModelRun, ModelFailure> {
        let (tx, rx) = mpsc::channel(1);
        let done = ModelEvent::Done {
            blocks: vec![Block::Text {
                text: format!("{} by {:?}", request.model, self.started.lock()),
            }],
            usage: Usage::default(),
            stop: StopReason::EndTurn,
        };
        tx.send(Ok(done)).await.unwrap();
        Ok(ModelRun::new(rx))
    }
}

#[async_trait]
impl KeyedProvider for Fake {
    async fn start(&self, url: &str, key: &str) -> Result<(), ModuleError> {
        if self.refuses {
            return Err(
                ModuleError::new(ModuleErrorKind::InvalidArgument, "bad key").with_field("api_key"),
            );
        }
        *self.started.lock() = Some((url.to_string(), key.to_string()));
        Ok(())
    }

    fn stop(&self) {
        *self.started.lock() = None;
    }
}

fn joined(first: &Arc<Fake>, second: &Arc<Fake>) -> Joined {
    Joined::new(vec![
        Section {
            name: "openrouter",
            endpoint: "https://first",
            keyed: |c| c.openrouter.as_ref(),
            provider: Arc::clone(first) as Arc<dyn KeyedProvider>,
        },
        Section {
            name: "zai",
            endpoint: "https://second",
            keyed: |c| c.zai.as_ref(),
            provider: Arc::clone(second) as Arc<dyn KeyedProvider>,
        },
    ])
}

fn keyed(key: &str, endpoint: &str) -> Option<Keyed> {
    Some(Keyed {
        api_key: key.to_string(),
        endpoint: endpoint.to_string(),
    })
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
        deadline: Instant::now() + Duration::from_secs(10),
        cancel: CancellationToken::new(),
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

async fn answer(joined: &Joined, model: &str) -> Result<String, ModelFailure> {
    let (blocks, _, _) = joined
        .generate(&scope(), request(model))
        .await?
        .collect()
        .await?;
    match blocks.first() {
        Some(Block::Text { text }) => Ok(text.clone()),
        other => panic!("not a text: {other:?}"),
    }
}

#[tokio::test]
async fn only_the_providers_with_a_key_serve_each_model_named_by_its_provider() {
    let (first, second) = (fake(&["a/one"], &[]), fake(&["glm"], &[]));
    let joined = joined(&first, &second);
    let config = ModelsConfig {
        zai: keyed("k2", "https://own"),
        openrouter: keyed("", ""),
        ..ModelsConfig::default()
    };
    joined.start(&config).await.unwrap();

    assert!(first.started.lock().is_none(), "no key: off");
    assert_eq!(
        *second.started.lock(),
        Some(("https://own".to_string(), "k2".to_string()))
    );
    let listed: Vec<_> = joined
        .models()
        .into_iter()
        .map(|m| (m.id, m.provider))
        .collect();
    assert_eq!(listed, [("glm".to_string(), "zai".to_string())]);
    assert!(answer(&joined, "glm").await.unwrap().contains("k2"));
    let refused = answer(&joined, "a/one").await.unwrap_err();
    assert_eq!(refused.kind, ModelFailureKind::Invalid);
}

#[tokio::test]
async fn a_request_goes_to_the_first_provider_listing_its_model_once_listed() {
    let (first, second) = (fake(&["shared"], &[]), fake(&["shared"], &["late"]));
    let joined = joined(&first, &second);
    let config = ModelsConfig {
        openrouter: keyed("k1", ""),
        zai: keyed("k2", ""),
        ..ModelsConfig::default()
    };
    joined.start(&config).await.unwrap();

    assert_eq!(
        *first.started.lock(),
        Some(("https://first".to_string(), "k1".to_string())),
        "no endpoint: the provider's own"
    );
    assert!(answer(&joined, "shared").await.unwrap().contains("k1"));
    // Not listed yet, served once the listing begun at start is in.
    assert!(answer(&joined, "late").await.unwrap().contains("k2"));
    let all: Vec<_> = joined
        .listed()
        .await
        .unwrap()
        .into_iter()
        .map(|m| (m.id, m.provider))
        .collect();
    assert_eq!(all.len(), 3);
    assert_eq!(all[2], ("late".to_string(), "zai".to_string()));
}

#[tokio::test]
async fn no_key_starts_with_no_model_and_a_refused_key_is_named_by_its_section() {
    let (first, second) = (fake(&["a"], &[]), fake(&["b"], &[]));
    let none = joined(&first, &second);
    none.start(&ModelsConfig::default()).await.unwrap();
    assert!(none.models().is_empty(), "no key: no provider on");
    assert!(first.started.lock().is_none() && second.started.lock().is_none());

    let refusing = Arc::new(Fake {
        now: Vec::new(),
        later: Vec::new(),
        refuses: true,
        started: Mutex::new(None),
    });
    let joined = joined(&first, &refusing);
    let config = ModelsConfig {
        openrouter: keyed("k1", ""),
        zai: keyed("bad", ""),
        ..ModelsConfig::default()
    };
    let refused = joined.start(&config).await.unwrap_err();
    assert_eq!(refused.field.as_deref(), Some("zai.api_key"));
    assert!(
        first.started.lock().is_none(),
        "a failed start stops the others"
    );
    assert!(joined.models().is_empty());
}
