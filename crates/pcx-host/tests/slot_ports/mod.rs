// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The slots' ports of `slots`: a scripted model provider, a context
//! engine and an agent.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use parking_lot::Mutex;
use pcx_module_api::agent::{TurnEnd, TurnEvent, TurnRequest, TurnStop, Turns};
use pcx_module_api::context::ContextEngine;
use pcx_module_api::model::{
    Block, Modality, ModelEvent, ModelFailure, ModelInfo, ModelKind, ModelMessage, ModelProvider,
    ModelRequest, ModelRun, Role, StopReason, ToolChoice, Usage,
};
use pcx_module_api::{ModuleError, ModuleErrorKind, ModuleHost, Scope};
use tokio::sync::mpsc;

/// The model the provider serves.
const MODEL: &str = "scripted-1";

/// The model it lists once it started, as a provider reads its listing
/// at start.
pub const STARTED_MODEL: &str = "scripted-started";

/// Answers "hello" to every request.
#[derive(Default)]
pub struct Scripted {
    /// Whether its module started.
    pub started: AtomicBool,
}

fn model(id: &str) -> ModelInfo {
    ModelInfo {
        id: id.to_string(),
        context_tokens: 1000,
        output_tokens: 100,
        kind: ModelKind::Chat as i32,
        input: vec![Modality::Text as i32],
        output: vec![Modality::Text as i32],
        ..ModelInfo::default()
    }
}

#[async_trait]
impl ModelProvider for Scripted {
    fn models(&self) -> Vec<ModelInfo> {
        let mut models = vec![model(MODEL)];
        if self.started.load(Ordering::Relaxed) {
            models.push(model(STARTED_MODEL));
        }
        models
    }

    async fn generate(
        &self,
        _scope: &Scope,
        _request: ModelRequest,
    ) -> Result<ModelRun, ModelFailure> {
        let (tx, rx) = mpsc::channel(4);
        tokio::spawn(async move {
            for event in [
                ModelEvent::TextDelta("hello".into()),
                ModelEvent::Done {
                    blocks: vec![Block::Text {
                        text: "hello".into(),
                    }],
                    usage: Usage {
                        input: 7,
                        output: 1,
                        cache_read: 0,
                        cache_write: 0,
                    },
                    stop: StopReason::EndTurn,
                },
            ] {
                if tx.send(Ok(event)).await.is_err() {
                    return;
                }
            }
        });
        Ok(ModelRun::new(rx))
    }
}

/// Recalls the session's name.
pub struct Recall;

#[async_trait]
impl ContextEngine for Recall {
    async fn recall(
        &self,
        _scope: &Scope,
        session: &str,
        _prompt: &str,
    ) -> Result<String, ModuleError> {
        Ok(format!("recalled {session}"))
    }

    async fn summarize(
        &self,
        _scope: &Scope,
        _session: &str,
        _model: &str,
        _messages: &[ModelMessage],
    ) -> Result<String, ModuleError> {
        Ok("summed".into())
    }
}

/// An agent whose turn recalls its context and asks the model once; a
/// turn of `wait` waits for its cancel and ends `Cancelled` after its step;
/// one of `tools` answers the tools it is listed.
pub struct Agent {
    pub host: Arc<Mutex<Option<Arc<dyn ModuleHost>>>>,
}

#[async_trait]
impl Turns for Agent {
    async fn run(
        &self,
        scope: &Scope,
        request: &TurnRequest,
        events: &mpsc::Sender<TurnEvent>,
    ) -> Result<TurnEnd, ModuleError> {
        let host =
            self.host.lock().clone().ok_or_else(|| {
                ModuleError::new(ModuleErrorKind::FailedPrecondition, "not started")
            })?;
        let usage = Usage {
            input: 3,
            output: 0,
            cache_read: 0,
            cache_write: 0,
        };
        if request.prompt == "tools" {
            let listed: Vec<String> = host
                .tools(scope)
                .await?
                .into_iter()
                .map(|t| t.tool.spec.name)
                .collect();
            return Ok(TurnEnd {
                stop: TurnStop::Answered(StopReason::EndTurn),
                steps: 0,
                usage,
                answer: listed.join(","),
            });
        }
        if request.prompt == "wait" {
            if events
                .send(TurnEvent::Text("waiting".into()))
                .await
                .is_err()
            {
                return Err(ModuleError::new(ModuleErrorKind::Internal, "no reader"));
            }
            scope.cancel.cancelled().await;
            return Ok(TurnEnd {
                stop: TurnStop::Cancelled,
                steps: 1,
                usage,
                answer: String::new(),
            });
        }
        let engine = host.context().await?.ok_or_else(|| {
            ModuleError::new(ModuleErrorKind::FailedPrecondition, "no context engine")
        })?;
        let recalled = engine
            .recall(scope, &request.session, &request.prompt)
            .await?;
        let asked = ModelRequest {
            model: MODEL.to_string(),
            system: request.instructions.clone(),
            messages: vec![ModelMessage {
                role: Role::User,
                blocks: vec![Block::Text {
                    text: request.prompt.clone(),
                }],
            }],
            tools: Vec::new(),
            max_tokens: 100,
            tool_choice: ToolChoice::Auto,
        };
        let mut run = host
            .model()
            .await?
            .generate(scope, asked)
            .await
            .map_err(|f| ModuleError::new(ModuleErrorKind::Unavailable, f.to_string()))?;
        let mut text = String::new();
        let mut used = Usage::default();
        while let Some(event) = run.next().await {
            match event
                .map_err(|f| ModuleError::new(ModuleErrorKind::Unavailable, f.to_string()))?
            {
                ModelEvent::TextDelta(delta) => {
                    text.push_str(&delta);
                    if events.send(TurnEvent::Text(delta)).await.is_err() {
                        return Err(ModuleError::new(ModuleErrorKind::Internal, "no reader"));
                    }
                }
                ModelEvent::Done { usage, .. } => used = usage,
                _ => {}
            }
        }
        Ok(TurnEnd {
            stop: TurnStop::Answered(StopReason::EndTurn),
            steps: 1,
            usage: used,
            answer: format!(
                "{text} [{recalled}] as {} with {}",
                scope.author.clone().unwrap_or_default(),
                request.instructions
            ),
        })
    }
}
