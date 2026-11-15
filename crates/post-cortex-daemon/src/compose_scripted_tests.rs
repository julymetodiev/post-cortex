// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A scripted model provider, as a module, for the daemon's tests of the
//! agent and the team.

use std::collections::VecDeque;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use pcx_module_api::model::{
    Block, Modality, ModelEvent, ModelFailure, ModelFailureKind, ModelInfo, ModelKind,
    ModelProvider, ModelRequest, ModelRun, StopReason, Usage,
};
use pcx_module_api::{
    API_VERSION, Event, HostContext, Manifest, Module, ModuleError, ModuleErrorKind, ModuleKind,
    Outcome, Resource, Scope,
};
use tokio::sync::mpsc;

/// A provider of `glm-5.3` whose answers follow a script; it keeps what it
/// was asked.
pub struct Scripted {
    manifest: Manifest,
    script: Mutex<VecDeque<Vec<Block>>>,
    /// What it was asked.
    pub asked: Mutex<Vec<ModelRequest>>,
    /// The answers it never gives: an empty one in the script holds its stream open.
    held: Mutex<Vec<mpsc::Sender<Result<ModelEvent, ModelFailure>>>>,
}

#[async_trait]
impl ModelProvider for Scripted {
    fn models(&self) -> Vec<ModelInfo> {
        vec![ModelInfo {
            id: "glm-5.3".into(),
            context_tokens: 100_000,
            output_tokens: 1000,
            kind: ModelKind::Chat as i32,
            input: vec![Modality::Text as i32],
            output: vec![Modality::Text as i32],
            ..ModelInfo::default()
        }]
    }

    async fn generate(&self, _: &Scope, request: ModelRequest) -> Result<ModelRun, ModelFailure> {
        self.asked.lock().push(request);
        let blocks = self
            .script
            .lock()
            .pop_front()
            .ok_or_else(|| ModelFailure::new(ModelFailureKind::Invalid, "no more script"))?;
        let stop = if blocks.iter().any(|b| matches!(b, Block::ToolUse { .. })) {
            StopReason::ToolUse
        } else {
            StopReason::EndTurn
        };
        let (tx, rx) = mpsc::channel(1);
        if blocks.is_empty() {
            self.held.lock().push(tx);
            return Ok(ModelRun::new(rx));
        }
        let done = ModelEvent::Done {
            blocks,
            usage: Usage::default(),
            stop,
        };
        tx.send(Ok(done))
            .await
            .map_err(|_| ModelFailure::new(ModelFailureKind::Unavailable, "dropped"))?;
        Ok(ModelRun::new(rx))
    }
}

/// The provider as a module.
pub struct Provider(pub Arc<Scripted>);

#[async_trait]
impl Module for Provider {
    fn manifest(&self) -> &Manifest {
        &self.0.manifest
    }
    async fn start(&self, _: HostContext) -> Result<(), ModuleError> {
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
        Some(Arc::clone(&self.0) as Arc<dyn ModelProvider>)
    }
}

impl Scripted {
    /// A provider of `glm-5.3` answering `script`, one answer per request.
    pub fn with(script: Vec<Vec<Block>>) -> Arc<Self> {
        Arc::new(Self {
            manifest: Manifest {
                id: "provider-test".into(),
                version: "0.1.0".into(),
                api: Some(API_VERSION),
                kind: ModuleKind::ModelProvider.into(),
                ..Manifest::default()
            },
            script: Mutex::new(VecDeque::from(script)),
            asked: Mutex::default(),
            held: Mutex::default(),
        })
    }
}
