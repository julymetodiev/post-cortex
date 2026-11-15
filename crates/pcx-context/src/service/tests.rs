// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The context engine over memory held in a list and a model that answers
//! a fixed text.

use std::time::{Duration, Instant};

use async_trait::async_trait;
use parking_lot::Mutex;
use pcx_module_api::model::{
    Block, ModelEvent, ModelFailure, ModelFailureKind, ModelInfo, ModelRequest, ModelRun, Role,
    StopReason, Usage,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::*;

#[derive(Default)]
struct Kept {
    summaries: Mutex<Vec<(String, String)>>,
}

#[async_trait]
impl Memory for Kept {
    async fn recall(&self, _: &str, query: &str) -> Result<(usize, String), ModuleError> {
        Ok(if query.contains("deploy") {
            (1, "We deploy with pcx-swap.".into())
        } else {
            (0, "## Context\n".into())
        })
    }

    async fn keep_summary(&self, session: &str, summary: &str) -> Result<(), ModuleError> {
        self.summaries
            .lock()
            .push((session.to_string(), summary.to_string()));
        Ok(())
    }
}

/// Answers `text`; keeps what it was asked.
struct Says {
    text: &'static str,
    asked: Mutex<Vec<ModelRequest>>,
}

#[async_trait]
impl ModelProvider for Says {
    fn models(&self) -> Vec<ModelInfo> {
        Vec::new()
    }

    async fn generate(&self, _: &Scope, request: ModelRequest) -> Result<ModelRun, ModelFailure> {
        self.asked.lock().push(request);
        let (tx, rx) = mpsc::channel(1);
        tx.send(Ok(ModelEvent::Done {
            blocks: vec![Block::Text {
                text: self.text.into(),
            }],
            usage: Usage::default(),
            stop: StopReason::EndTurn,
        }))
        .await
        .map_err(|_| ModelFailure::new(ModelFailureKind::Unavailable, "dropped"))?;
        Ok(ModelRun::new(rx))
    }
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

#[tokio::test]
async fn recall_adds_only_what_memory_found_and_a_summary_is_kept() {
    let memory = Kept::default();
    let engine = Engine { memory: &memory };
    assert_eq!(
        engine.recall("s1", "how do we deploy?").await.unwrap(),
        "We deploy with pcx-swap."
    );
    assert_eq!(engine.recall("s1", "hello").await.unwrap(), "");

    let model = Says {
        text: "They set up the board.",
        asked: Mutex::default(),
    };
    let said = vec![ModelMessage {
        role: Role::User,
        blocks: vec![Block::Text {
            text: "make a board".into(),
        }],
    }];
    let summary = engine
        .summarize(&model, &scope(), "s1", "glm", &said)
        .await
        .unwrap();
    assert_eq!(summary, "They set up the board.");
    assert_eq!(
        *memory.summaries.lock(),
        [("s1".to_string(), "They set up the board.".to_string())]
    );
    assert_eq!(model.asked.lock()[0].messages.len(), 2);

    let silent = Says {
        text: "  ",
        asked: Mutex::default(),
    };
    let failed = engine
        .summarize(&silent, &scope(), "s1", "glm", &said)
        .await
        .unwrap_err();
    assert_eq!(failed.kind, ModuleErrorKind::Unavailable);
}
