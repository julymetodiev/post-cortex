// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A turn over ports held in memory: a scripted model, tools that echo,
//! hooks that record (and may block), a transcript in a list.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use parking_lot::Mutex;
use pcx_module_api::model::{
    Modality, ModelEvent, ModelFailure, ModelFailureKind, ModelInfo, ModelKind, ModelRun,
    ModelTool, StopReason,
};
use serde_json::json;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::ports::ToolAnswer;
use pcx_module_api::context::ContextEngine;

/// One scripted answer, or its failure.
type Answer = Result<(Vec<Block>, StopReason), ModelFailure>;

/// Answers each request with the next of its script; keeps the requests.
#[derive(Default)]
struct Scripted {
    script: Mutex<VecDeque<Answer>>,
    asked: Mutex<Vec<ModelRequest>>,
    /// The window of its model `glm`; 0: it lists no model.
    window: u64,
}

impl Scripted {
    fn with(script: Vec<Answer>) -> Self {
        Self {
            script: Mutex::new(script.into()),
            ..Self::default()
        }
    }
}

#[async_trait]
impl ModelProvider for Scripted {
    fn models(&self) -> Vec<ModelInfo> {
        if self.window == 0 {
            return Vec::new();
        }
        vec![ModelInfo {
            id: "glm".into(),
            context_tokens: self.window,
            output_tokens: 100,
            kind: ModelKind::Chat as i32,
            input: vec![Modality::Text as i32],
            output: vec![Modality::Text as i32],
            ..ModelInfo::default()
        }]
    }

    async fn generate(&self, _: &Scope, request: ModelRequest) -> Result<ModelRun, ModelFailure> {
        self.asked.lock().push(request);
        let next = self.script.lock().pop_front().unwrap_or_else(|| {
            Err(ModelFailure::new(
                ModelFailureKind::Invalid,
                "no more script",
            ))
        });
        let (blocks, stop) = next?;
        let (tx, rx) = mpsc::channel(4);
        let text = domain::text_of(&blocks);
        if !text.is_empty() {
            tx.send(Ok(ModelEvent::TextDelta(text))).await.unwrap();
        }
        let usage = Usage {
            input: 10,
            output: 5,
            ..Usage::default()
        };
        tx.send(Ok(ModelEvent::Done {
            blocks,
            usage,
            stop,
        }))
        .await
        .unwrap();
        Ok(ModelRun::new(rx))
    }
}

/// Echoes a tool's input; keeps the calls.
#[derive(Default)]
struct Echo {
    called: Mutex<Vec<String>>,
}

#[async_trait]
impl Tools for Echo {
    async fn list(&self) -> Result<Vec<(String, ModelTool)>, ModuleError> {
        Ok(vec![(
            "tools-fs".into(),
            ModelTool {
                name: "fs_read".into(),
                description: "Read a file".into(),
                input_schema: serde_json::Map::new(),
            },
        )])
    }

    async fn call(&self, name: &str, input: Value) -> ToolAnswer {
        self.called.lock().push(name.to_string());
        ToolAnswer {
            content: format!("{name} {input}"),
            is_error: false,
            offline: false,
        }
    }
}

/// Records the events; blocks the one named `blocks`.
#[derive(Default)]
struct Heard {
    events: Mutex<Vec<String>>,
    blocks: Option<&'static str>,
}

#[async_trait]
impl Hooks for Heard {
    async fn send(&self, _: EventKind, event: Event) -> Result<Outcome, ModuleError> {
        self.events.lock().push(event.hook_event_name.clone());
        Ok(if self.blocks == Some(event.hook_event_name.as_str()) {
            Outcome {
                decision: "block".into(),
                reason: "not now".into(),
                ..Outcome::default()
            }
        } else {
            Outcome::default()
        })
    }
}

/// Turns in a list.
#[derive(Default)]
struct Kept {
    turns: Mutex<Vec<Turn>>,
}

#[async_trait]
impl Transcript for Kept {
    async fn load(&self, _: &str) -> Result<Vec<Turn>, ModuleError> {
        Ok(self.turns.lock().clone())
    }

    async fn append(&self, _: &str, turns: &[Turn]) -> Result<(), ModuleError> {
        self.turns.lock().extend_from_slice(turns);
        Ok(())
    }
}

fn settings() -> Settings {
    Settings {
        model: "glm".into(),
        system: "Be brief.".into(),
        max_tokens: 100,
        max_steps: 5,
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
        deadline: Instant::now() + Duration::from_secs(10),
        cancel: CancellationToken::new(),
    }
}

fn read_call() -> (Vec<Block>, StopReason) {
    (
        vec![Block::ToolUse {
            id: "c1".into(),
            name: "fs_read".into(),
            input: json!({ "path": "README.md" }),
        }],
        StopReason::ToolUse,
    )
}

fn said(text: &str) -> (Vec<Block>, StopReason) {
    (vec![Block::Text { text: text.into() }], StopReason::EndTurn)
}

async fn turn(
    model: &Scripted,
    tools: &Echo,
    hooks: &Heard,
    kept: &Kept,
    prompt: &str,
) -> (TurnEnd, Vec<TurnEvent>) {
    let (scope, settings) = (scope(), settings());
    let (tx, mut rx) = mpsc::channel(64);
    let agent = Agent {
        models: model,
        tools,
        hooks,
        transcript: kept,
        context: None,
        settings: &settings,
        scope: &scope,
    };
    let end = agent
        .run(&TurnRequest::new("s1", prompt), &tx)
        .await
        .unwrap();
    drop(tx);
    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        events.push(event);
    }
    (end, events)
}

#[tokio::test]
async fn a_tool_turn_runs_the_tool_and_keeps_every_turn() {
    let model = Scripted::with(vec![Ok(read_call()), Ok(said("It is about pcx."))]);
    let (tools, hooks, kept) = (Echo::default(), Heard::default(), Kept::default());
    let (end, events) = turn(&model, &tools, &hooks, &kept, "What is in README.md?").await;

    assert_eq!(end.stop, TurnStop::Answered(StopReason::EndTurn));
    assert_eq!((end.steps, end.usage.input, end.usage.output), (2, 20, 10));
    assert_eq!(*tools.called.lock(), ["fs_read"]);
    assert_eq!(
        *hooks.events.lock(),
        [
            "UserPromptSubmit",
            "PreModelCall",
            "PostModelCall",
            "PreToolUse",
            "PostToolUse",
            "PreModelCall",
            "PostModelCall",
            "Stop"
        ]
    );
    assert!(matches!(&events[0], TurnEvent::ToolCall { name, .. } if name == "fs_read"));
    assert!(matches!(
        &events[1],
        TurnEvent::ToolResult {
            is_error: false,
            ..
        }
    ));
    assert_eq!(events[2], TurnEvent::Text("It is about pcx.".into()));

    // The prompt, the call, its result, the answer and the turn's end, each
    // following the last.
    let turns = kept.turns.lock().clone();
    let roles: Vec<Role> = turns.iter().map(|t| t.role).collect();
    assert_eq!(
        roles,
        [
            Role::User,
            Role::Assistant,
            Role::User,
            Role::Assistant,
            Role::Assistant
        ]
    );
    assert_eq!(
        turns[4].blocks,
        [Block::End {
            stop: "end_turn".into(),
            detail: String::new()
        }]
    );
    assert!(turns[4].ends());
    for pair in turns.windows(2) {
        assert_eq!(pair[1].parent.as_deref(), Some(pair[0].turn_id.as_str()));
    }
    // The second call carried the tool's result.
    let second = &model.asked.lock()[1];
    assert_eq!(second.messages.len(), 3);
    assert!(
        matches!(&second.messages[2].blocks[0], Block::ToolResult { tool_use_id, .. } if tool_use_id == "c1")
    );
    assert_eq!(second.system, "Be brief.");
}

#[tokio::test]
async fn a_blocked_tool_is_not_run_and_a_blocked_prompt_is_not_kept() {
    let model = Scripted::with(vec![Ok(read_call()), Ok(said("Fine."))]);
    let (tools, kept) = (Echo::default(), Kept::default());
    let hooks = Heard {
        blocks: Some("PreToolUse"),
        ..Heard::default()
    };
    let (end, events) = turn(&model, &tools, &hooks, &kept, "Read it").await;
    assert_eq!(end.stop, TurnStop::Answered(StopReason::EndTurn));
    assert!(tools.called.lock().is_empty());
    assert!(
        matches!(&events[1], TurnEvent::ToolResult { is_error: true, content, .. } if content == "Blocked: not now")
    );

    let hooks = Heard {
        blocks: Some("UserPromptSubmit"),
        ..Heard::default()
    };
    let kept = Kept::default();
    let (end, _) = turn(&Scripted::default(), &tools, &hooks, &kept, "No").await;
    assert_eq!(end.stop, TurnStop::Blocked("not now".into()));
    assert!(kept.turns.lock().is_empty());
}

#[tokio::test]
async fn a_failure_that_passes_is_retried_and_one_that_does_not_ends_the_turn() {
    let mut busy = ModelFailure::new(ModelFailureKind::Overloaded, "busy");
    busy.retry_after = Some(Duration::from_millis(1));
    let model = Scripted::with(vec![Err(busy), Ok(said("Done."))]);
    let (tools, hooks, kept) = (Echo::default(), Heard::default(), Kept::default());
    let (end, events) = turn(&model, &tools, &hooks, &kept, "Go").await;
    assert_eq!(end.stop, TurnStop::Answered(StopReason::EndTurn));
    assert!(
        matches!(&events[0], TurnEvent::Retrying { after, .. } if *after == Duration::from_millis(1))
    );

    let spent = ModelFailure::new(ModelFailureKind::QuotaExhausted, "plan used up");
    let model = Scripted::with(vec![Err(spent.clone())]);
    let (end, _) = turn(&model, &tools, &hooks, &Kept::default(), "Go").await;
    assert_eq!(end.stop, TurnStop::Failed(spent));
    assert_eq!(model.asked.lock().len(), 1, "not retried");
}

#[tokio::test]
async fn a_session_resumes_from_its_transcript_and_a_turn_has_a_step_limit() {
    let model = Scripted::with(vec![Ok(said("Hello.")), Ok(said("Again."))]);
    let (tools, hooks, kept) = (Echo::default(), Heard::default(), Kept::default());
    turn(&model, &tools, &hooks, &kept, "Hi").await;
    turn(&model, &tools, &hooks, &kept, "And now?").await;
    let resumed = model.asked.lock()[1].clone();
    let said: Vec<String> = resumed
        .messages
        .iter()
        .map(|m| domain::text_of(&m.blocks))
        .collect();
    assert_eq!(said, ["Hi", "Hello.", "And now?"]);

    let looping = Scripted::with((0..6).map(|_| Ok(read_call())).collect());
    let (end, _) = turn(&looping, &tools, &hooks, &Kept::default(), "Loop").await;
    assert_eq!((end.stop, end.steps), (TurnStop::StepLimit, 5));
}

/// Recalls a line; sums up a conversation as how many messages it had.
#[derive(Default)]
struct Engine {
    summed: Mutex<Vec<usize>>,
}

#[async_trait]
impl ContextEngine for Engine {
    async fn recall(&self, _: &Scope, _: &str, _: &str) -> Result<String, ModuleError> {
        Ok("Memory: we deploy with pcx-swap.".into())
    }

    async fn summarize(
        &self,
        _: &Scope,
        _: &str,
        _: &str,
        messages: &[pcx_module_api::model::ModelMessage],
    ) -> Result<String, ModuleError> {
        self.summed.lock().push(messages.len());
        Ok(format!("{} messages, summed up", messages.len()))
    }
}

#[tokio::test]
async fn recall_goes_with_the_prompt_once_and_a_long_conversation_is_compacted_once() {
    let long = ModelFailure::new(ModelFailureKind::ContextTooLong, "too long");
    let model = Scripted::with(vec![
        Ok(said("Hello.")),
        Err(long.clone()),
        Ok(said("Deployed.")),
    ]);
    let (tools, hooks, kept, engine) = (
        Echo::default(),
        Heard::default(),
        Kept::default(),
        Engine::default(),
    );
    let (scope, settings) = (scope(), settings());
    let (tx, _rx) = mpsc::channel(64);
    let agent = Agent {
        models: &model,
        tools: &tools,
        hooks: &hooks,
        transcript: &kept,
        context: Some(&engine),
        settings: &settings,
        scope: &scope,
    };
    agent.run(&TurnRequest::new("s1", "Hi"), &tx).await.unwrap();
    let end = agent
        .run(&TurnRequest::new("s1", "Deploy it"), &tx)
        .await
        .unwrap();
    assert_eq!(end.stop, TurnStop::Answered(StopReason::EndTurn));

    let asked = model.asked.lock().clone();
    // The system prompt is the same each turn: the recall goes with the
    // first prompt, and the same recall is not added again.
    assert_eq!(asked[0].system, asked[1].system);
    assert!(!asked[0].system.contains("pcx-swap"));
    let first = domain::text_of(&asked[0].messages[0].blocks);
    assert!(first.starts_with("Context for this turn"), "{first}");
    assert!(
        first.ends_with("Memory: we deploy with pcx-swap.\nHi"),
        "{first}"
    );
    assert_eq!(
        domain::text_of(&asked[1].messages[2].blocks),
        "Deploy it",
        "the context the conversation carries is not added again"
    );
    // The turn's start is the request's start of the next turn: its cache.
    assert_eq!(asked[1].messages[..1], asked[0].messages[..]);
    assert_eq!(
        kept.turns.lock()[0].text,
        "Hi",
        "search reads the prompt alone"
    );
    // Summed up: the two turns before the prompt; then asked from the summary.
    assert_eq!(*engine.summed.lock(), [2]);
    let resumed: Vec<String> = asked[2]
        .messages
        .iter()
        .map(|m| domain::text_of(&m.blocks))
        .collect();
    assert_eq!(
        resumed,
        [
            format!("{}\n2 messages, summed up", domain::SUMMARY_HEAD),
            "Deploy it".to_string()
        ]
    );
    // The summary is a turn of the transcript, and the next turn starts there.
    assert!(kept.turns.lock().iter().any(Turn::summarizes));

    // A second compaction in one turn is not tried.
    let model = Scripted::with(vec![Err(long.clone()), Err(long.clone())]);
    let agent = Agent {
        models: &model,
        ..agent
    };
    let end = agent
        .run(&TurnRequest::new("s1", "Again"), &tx)
        .await
        .unwrap();
    assert_eq!(end.stop, TurnStop::Failed(long));
}

mod asker;
mod tools;
mod window;

#[tokio::test]
async fn an_answer_the_most_tokens_cut_goes_on_and_stops_going_on() {
    let cut = || Ok((Vec::new(), StopReason::MaxTokens));
    let model = Scripted::with(vec![cut(), Ok(said("In smaller steps."))]);
    let (tools, hooks, kept) = (Echo::default(), Heard::default(), Kept::default());
    let (end, _) = turn(&model, &tools, &hooks, &kept, "Write it").await;
    assert_eq!(end.stop, TurnStop::Answered(StopReason::EndTurn));
    assert_eq!(end.answer, "In smaller steps.");
    assert!(
        kept.turns
            .lock()
            .iter()
            .any(|t| t.role == Role::User && t.text == domain::CUT_NOTE)
    );

    let model = Scripted::with(vec![cut(), cut(), cut(), Ok(said("Never asked."))]);
    let (tools, hooks, kept) = (Echo::default(), Heard::default(), Kept::default());
    let (end, _) = turn(&model, &tools, &hooks, &kept, "Write it").await;
    assert_eq!(end.stop, TurnStop::Answered(StopReason::MaxTokens));
    assert_eq!(end.steps, 3);
}
