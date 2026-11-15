// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The agent service and the model call: the model asked with a request,
//! its deltas told as they come, its failures recovered from by their kind
//! (retried after a wait, the conversation compacted, or the turn failed);
//! a stopped turn keeps the text the model wrote until then.

use pcx_module_api::agent::TurnEvent;
use pcx_module_api::model::{
    Block, ModelEvent, ModelFailure, ModelFailureKind, ModelRequest, StopReason, Usage,
};
use tokio::sync::mpsc;

use crate::domain::{Recover, Recovery};
use crate::service::{Agent, tell};

/// The longest a model's stream may stay silent before it is taken as lost: a
/// model that thinks long still sends what it thinks, and a first answer took
/// 388 s of a long turn.
const QUIET: std::time::Duration = std::time::Duration::from_secs(600);

impl Agent<'_> {
    /// Ask the model, streaming its deltas, recovering from its failures by
    /// their kind.
    pub async fn ask(
        &self,
        request: ModelRequest,
        events: &mpsc::Sender<TurnEvent>,
    ) -> Result<(Vec<Block>, Usage, StopReason), Asked> {
        let mut recovery = Recovery::default();
        loop {
            let mut said = String::new();
            let failure = match self.stream(request.clone(), events, &mut said).await {
                Ok(answer) => return Ok(answer),
                // A stopped turn ends its model call: not a failure.
                Err(_) if self.scope.cancel.is_cancelled() => return Err(Asked::Cancelled(said)),
                Err(failure) => failure,
            };
            match recovery.after(&failure) {
                Recover::Retry(after) => {
                    tell(
                        events,
                        TurnEvent::Retrying {
                            failure: failure.clone(),
                            after,
                        },
                    )
                    .await;
                    tokio::select! {
                        () = self.scope.cancel.cancelled() => return Err(Asked::Cancelled(String::new())),
                        () = tokio::time::sleep(after) => {}
                    }
                }
                Recover::Compact => return Err(Asked::Compact(failure)),
                Recover::Stop => return Err(Asked::Failed(failure)),
            }
        }
    }

    /// One model call: its deltas to `events` (its text so far in `said`),
    /// its answer.
    pub async fn stream(
        &self,
        request: ModelRequest,
        events: &mpsc::Sender<TurnEvent>,
        said: &mut String,
    ) -> Result<(Vec<Block>, Usage, StopReason), ModelFailure> {
        let stopped =
            || ModelFailure::new(ModelFailureKind::Unavailable, "a person stopped the turn");
        let mut run = tokio::select! {
            () = self.scope.cancel.cancelled() => return Err(stopped()),
            run = self.models.generate(self.scope, request) => run?,
        };
        // A provider that hangs does not hold a turn someone stopped.
        loop {
            let event = tokio::select! {
                () = self.scope.cancel.cancelled() => return Err(stopped()),
                () = tokio::time::sleep(QUIET) => return Err(ModelFailure::new(
                    ModelFailureKind::Unavailable,
                    format!("the model sent nothing for {} s", QUIET.as_secs()),
                )),
                event = run.next() => event,
            };
            let Some(event) = event else { break };
            match event? {
                ModelEvent::TextDelta(text) => {
                    said.push_str(&text);
                    tell(events, TurnEvent::Text(text)).await;
                }
                ModelEvent::ThinkingDelta(text) => tell(events, TurnEvent::Thinking(text)).await,
                // Told whole when it runs.
                ModelEvent::ToolUse { .. } => {}
                ModelEvent::Done {
                    blocks,
                    usage,
                    stop,
                } => return Ok((blocks, usage, stop)),
            }
        }
        Err(ModelFailure::new(
            ModelFailureKind::Unavailable,
            "the answer ended before it was done",
        ))
    }
}

/// Why a model call gave no answer.
pub enum Asked {
    /// The conversation is too long for the model: compact it.
    Compact(ModelFailure),
    /// It failed past its recovery.
    Failed(ModelFailure),
    /// A person stopped the turn: the text the model wrote until then.
    Cancelled(String),
}
