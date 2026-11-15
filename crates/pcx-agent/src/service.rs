// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The agent service: one turn over its ports (ARCHITECTURE section 7, the
//! agent). The prompt is kept, the model asked with the conversation and
//! the tools, each tool call it makes run and answered, until it answers;
//! every turn of it is kept in the transcript, and the modules that answer
//! the agent's events hear each step (a blocking answer stops the turn, or,
//! before a tool, that tool).

use pcx_module_api::context::ContextEngine;
use pcx_module_api::model::{
    Block, ModelProvider, ModelRequest, ModelTool, Role, StopReason, ToolChoice, Usage,
};
use pcx_module_api::{Event, EventKind, ModuleError, ModuleErrorKind, Outcome, Scope, json};
use serde_json::Value;
use tokio::sync::mpsc;

use crate::domain::{self, AGENT, Call, Next, Turn};
use crate::ports::{Hooks, Tools, Transcript};
use pcx_module_api::agent::{TurnEnd, TurnEvent, TurnRequest, TurnStop};

/// An outcome's answer that blocks.
const BLOCK: &str = "block";

/// What the agent runs with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// The model's id.
    pub model: String,
    /// The system prompt.
    pub system: String,
    /// The most tokens of one answer.
    pub max_tokens: u32,
    /// The most model calls of one turn.
    pub max_steps: u32,
}

/// The agent over its ports, in one call's scope.
pub struct Agent<'a> {
    /// The models.
    pub models: &'a dyn ModelProvider,
    /// The tools it may call.
    pub tools: &'a dyn Tools,
    /// The modules that answer its events.
    pub hooks: &'a dyn Hooks,
    /// The session's turns.
    pub transcript: &'a dyn Transcript,
    /// The context engine; none: the conversation goes as it is.
    pub context: Option<&'a dyn ContextEngine>,
    /// How it runs.
    pub settings: &'a Settings,
    /// The call's scope (its deadline and cancellation).
    pub scope: &'a Scope,
}

/// A new turn id.
fn turn_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn blocked(outcome: &Outcome) -> Option<String> {
    (outcome.decision == BLOCK).then(|| outcome.reason.clone())
}

fn add(total: &mut Usage, more: Usage) {
    total.input += more.input;
    total.output += more.output;
    total.cache_read += more.cache_read;
    total.cache_write += more.cache_write;
}

/// The end of a turn, with its last answer.
fn ended(stop: TurnStop, steps: u32, usage: Usage, answer: String) -> TurnEnd {
    TurnEnd {
        stop,
        steps,
        usage,
        answer,
    }
}

impl Agent<'_> {
    /// Run the turn `request` asks, its events sent to `events` as they
    /// happen (a caller that stopped listening does not stop it): its
    /// instructions go with the system prompt, its model (else the
    /// agent's) answers, with the tools it allows.
    ///
    /// # Errors
    /// The transcript or the host failed.
    pub async fn run(
        &self,
        request: &TurnRequest,
        events: &mpsc::Sender<TurnEvent>,
    ) -> Result<TurnEnd, ModuleError> {
        let (session, prompt) = (request.session.as_str(), request.prompt.as_str());
        // Who the agent's turns are written as (W25): the asker writes the
        // prompt.
        let speaker = Some(request.speaker.clone().unwrap_or_else(|| AGENT.to_string()));
        let model = request
            .model
            .clone()
            .unwrap_or_else(|| self.settings.model.clone());
        let mut turns = self.transcript.load(session).await?;
        let started = self
            .send(
                EventKind::TurnStart,
                Event {
                    hook_event_name: "UserPromptSubmit".into(),
                    session_id: session.into(),
                    prompt: prompt.into(),
                    ..Event::default()
                },
            )
            .await?;
        if let Some(reason) = blocked(&started) {
            return Ok(ended(
                TurnStop::Blocked(reason),
                0,
                Usage::default(),
                String::new(),
            ));
        }
        // The system prompt is the agent's and the turn's instructions
        // only, the same from turn to turn: the provider's cache serves the
        // request's start.
        let mut system = self.settings.system.clone();
        if !request.instructions.is_empty() {
            system = format!("{system}\n\n{}", request.instructions);
        }
        // What the modules add to the turn (cards) and what memory recalls
        // for the prompt go with the prompt, once: what the conversation
        // carries already is not added again.
        let mut context = Vec::new();
        if let Some(added) = started
            .hook_specific_output
            .filter(|o| !o.additional_context.is_empty())
        {
            context.push(added.additional_context);
        }
        if let Some(engine) = self.context {
            let recalled = engine.recall(self.scope, session, prompt).await?;
            if !recalled.is_empty() {
                context.push(recalled);
            }
        }
        let block =
            domain::context_block(&context.join("\n\n")).filter(|b| !domain::carries(&turns, b));
        // Who asks writes the prompt: the person a module's turn is for, else
        // the scope's author.
        let asker = request.asker.clone().or_else(|| self.scope.author.clone());
        self.keep_turn(session, &mut turns, |id, parent| {
            domain::prompt_turn(id, parent, block, prompt).by(asker)
        })
        .await?;
        let tools = domain::allowed(
            self.tools.list().await?,
            &request.modules,
            &request.tools,
            &request.withheld,
        );

        let mut usage = Usage::default();
        let mut steps = 0;
        // A turn compacts its conversation once, before the model refuses
        // it when it can.
        let mut compacted = false;
        let mut deferrals = 0;
        let mut called = false;
        let mut cuts = 0;
        let mut failures = domain::Failures::default();
        let stop = loop {
            if steps == self.settings.max_steps {
                break TurnStop::StepLimit;
            }
            if self.scope.cancel.is_cancelled() {
                break TurnStop::Cancelled;
            }
            if !compacted && self.due(&model, &system, &turns, &tools, called, &mut deferrals) {
                compacted = self
                    .compact(session, &model, &mut turns, speaker.clone())
                    .await?;
            }
            let before = self
                .send(
                    EventKind::PreModelCall,
                    model_event("PreModelCall", session),
                )
                .await?;
            if let Some(reason) = blocked(&before) {
                break TurnStop::Blocked(reason);
            }
            steps += 1;
            let request = ModelRequest {
                model: model.clone(),
                system: system.clone(),
                messages: domain::conversation(&turns),
                tools: tools.clone(),
                max_tokens: self.settings.max_tokens,
                tool_choice: ToolChoice::Auto,
            };
            let (blocks, used, stop) = match self.ask(request, events).await {
                Ok(answer) => answer,
                Err(Asked::Compact(failure)) if !compacted => {
                    compacted = true;
                    if self
                        .compact(session, &model, &mut turns, speaker.clone())
                        .await?
                    {
                        continue;
                    }
                    break TurnStop::Failed(failure);
                }
                Err(Asked::Compact(failure) | Asked::Failed(failure)) => {
                    break TurnStop::Failed(failure);
                }
                // What it wrote until a person stopped it stays.
                Err(Asked::Cancelled(said)) => {
                    if !said.is_empty() {
                        self.keep(
                            session,
                            &mut turns,
                            Role::Assistant,
                            vec![Block::Text { text: said }],
                            speaker.clone(),
                        )
                        .await?;
                    }
                    break TurnStop::Cancelled;
                }
            };
            add(&mut usage, used);
            let after = self
                .send(
                    EventKind::PostModelCall,
                    model_event("PostModelCall", session),
                )
                .await?;
            let goes_on = stop == StopReason::MaxTokens && cuts < domain::MAX_CUTS;
            let answer = if goes_on {
                domain::cut_answer(blocks.clone())
            } else {
                blocks.clone()
            };
            self.keep(
                session,
                &mut turns,
                Role::Assistant,
                answer,
                speaker.clone(),
            )
            .await?;
            if let Some(reason) = blocked(&after) {
                break TurnStop::Blocked(reason);
            }
            match domain::next(&blocks, stop) {
                Next::End(StopReason::MaxTokens) if goes_on => {
                    cuts += 1;
                    let note = vec![Block::Text {
                        text: domain::CUT_NOTE.to_string(),
                    }];
                    self.keep(session, &mut turns, Role::User, note, speaker.clone())
                        .await?;
                }
                Next::End(stop) => break TurnStop::Answered(stop),
                Next::Tools(calls) => {
                    called = true;
                    let mut results = Vec::with_capacity(calls.len());
                    let mut away = None;
                    for call in calls {
                        let (result, offline) =
                            self.run_tool(session, call, &tools, events).await?;
                        if offline && let Block::ToolResult { content, .. } = &result {
                            away = Some(content.clone());
                        }
                        results.push(result);
                    }
                    let going_round = failures.see(&results);
                    self.keep(session, &mut turns, Role::User, results, speaker.clone())
                        .await?;
                    // Every call is answered; the work waits for the runner
                    // and goes on from here, its model not asked to try again.
                    if let Some(why) = away {
                        return Err(ModuleError::new(ModuleErrorKind::Offline, why));
                    }
                    if let Some(what) = going_round {
                        break TurnStop::Repeating(what);
                    }
                }
            }
        };
        let answer = turns
            .iter()
            .rev()
            .find(|t| t.role == Role::Assistant)
            .map(|t| t.text.clone())
            .unwrap_or_default();
        // Its end stays in the transcript (W25): what a person reading it
        // later sees of how it stopped.
        self.keep(
            session,
            &mut turns,
            Role::Assistant,
            vec![domain::end_block(&stop)],
            speaker,
        )
        .await?;
        self.send(
            EventKind::TurnEnd,
            Event {
                hook_event_name: "Stop".into(),
                session_id: session.into(),
                last_assistant_message: answer.clone(),
                ..Event::default()
            },
        )
        .await?;
        Ok(ended(stop, steps, usage, answer))
    }

    /// Run `call` unless it names a tool the turn was not given (`given`)
    /// or a module blocks it; its result block.
    async fn run_tool(
        &self,
        session: &str,
        call: Call,
        given: &[ModelTool],
        events: &mpsc::Sender<TurnEvent>,
    ) -> Result<(Block, bool), ModuleError> {
        tell(
            events,
            TurnEvent::ToolCall {
                id: call.id.clone(),
                name: call.name.clone(),
                input: call.input.clone(),
            },
        )
        .await;
        let input = match &call.input {
            Value::Object(fields) => Some(json::object_to_wire(fields.clone())),
            _ => None,
        };
        let before = self
            .send(
                EventKind::PreTool,
                Event {
                    hook_event_name: "PreToolUse".into(),
                    session_id: session.into(),
                    tool_name: call.name.clone(),
                    tool_input: input.clone(),
                    ..Event::default()
                },
            )
            .await?;
        let answer = match blocked(&before) {
            Some(reason) => crate::ports::ToolAnswer {
                content: format!("Blocked: {reason}"),
                is_error: true,
                offline: false,
            },
            None if !given.iter().any(|t| t.name == call.name) => crate::ports::ToolAnswer {
                content: format!("{} is not a tool of this turn", call.name),
                is_error: true,
                offline: false,
            },
            None => {
                // Stopped, a call still gets its result: the conversation
                // the member goes on from keeps every call answered.
                tokio::select! {
                    () = self.scope.cancel.cancelled() => crate::ports::ToolAnswer {
                        content: "A person stopped the turn while this ran.".to_string(),
                        is_error: true,
                        offline: false,
                    },
                    answer = self.tools.call(&call.name, call.input.clone()) => crate::ports::ToolAnswer {
                        content: domain::shown(answer.content),
                        is_error: answer.is_error,
                        offline: answer.offline,
                    },
                }
            }
        };
        self.send(
            EventKind::PostTool,
            Event {
                hook_event_name: "PostToolUse".into(),
                session_id: session.into(),
                tool_name: call.name.clone(),
                tool_input: input,
                tool_response: Some(json::to_wire(Value::String(answer.content.clone()))),
                ..Event::default()
            },
        )
        .await?;
        tell(
            events,
            TurnEvent::ToolResult {
                id: call.id.clone(),
                content: answer.content.clone(),
                is_error: answer.is_error,
            },
        )
        .await;
        Ok((
            Block::ToolResult {
                tool_use_id: call.id,
                content: answer.content,
                is_error: answer.is_error,
            },
            answer.offline,
        ))
    }

    /// Keep a turn of `role` with `blocks` by `author`, following the
    /// newest one.
    async fn keep(
        &self,
        session: &str,
        turns: &mut Vec<Turn>,
        role: Role,
        blocks: Vec<Block>,
        author: Option<String>,
    ) -> Result<(), ModuleError> {
        self.keep_turn(session, turns, |id, parent| {
            Turn::new(id, role, parent, blocks).by(author)
        })
        .await
    }

    /// Keep the turn `make` builds from a new id and the turn it follows
    /// (the newest of `turns`) in the transcript, and in `turns`.
    async fn keep_turn(
        &self,
        session: &str,
        turns: &mut Vec<Turn>,
        make: impl FnOnce(String, Option<String>) -> Turn,
    ) -> Result<(), ModuleError> {
        let turn = make(turn_id(), turns.last().map(|t| t.turn_id.clone()));
        self.transcript
            .append(session, std::slice::from_ref(&turn))
            .await?;
        turns.push(turn);
        Ok(())
    }

    async fn send(&self, kind: EventKind, event: Event) -> Result<Outcome, ModuleError> {
        self.hooks.send(kind, event).await
    }
}

/// A model call's event (no hook names it).
fn model_event(name: &str, session: &str) -> Event {
    Event {
        hook_event_name: name.into(),
        session_id: session.into(),
        ..Event::default()
    }
}

/// Send `event` to the turn's caller; one that stopped listening misses it.
async fn tell(events: &mpsc::Sender<TurnEvent>, event: TurnEvent) {
    if events.send(event).await.is_err() {
        tracing::debug!("the turn's caller stopped listening");
    }
}

mod ask;
mod window;

use ask::Asked;

#[cfg(test)]
mod tests;
