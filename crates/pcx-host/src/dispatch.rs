// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Event dispatch (section 7): every subscribed module answers under its
//! subscription's deadline and failure policy; their outcomes merge into
//! one, in load order.

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::future::join_all;
use pcx_module_api::{
    Event, EventKind, FailurePolicy, HookSpecificOutput, ModuleError, Outcome, Scope, Subscription,
};

use crate::grant;
use crate::policy::Guarded;

/// A subscription's deadline when its manifest sets none.
const DEFAULT_EVENT_DEADLINE: Duration = Duration::from_secs(5);

/// The decision that blocks the turn (Claude Code's hook answer).
const BLOCK: &str = "block";

/// A module that failed an event.
#[derive(Debug, Clone, PartialEq)]
pub struct Failure {
    /// The module.
    pub module: String,
    /// Why.
    pub error: ModuleError,
    /// Whether its failure blocked the turn (fail-closed).
    pub blocked: bool,
}

/// The merged answer of every subscribed module.
#[derive(Debug, Clone, PartialEq)]
pub struct Dispatched {
    /// The merged outcome: blocked if any module blocked or a fail-closed
    /// one failed; reasons and added context joined in load order.
    pub outcome: Outcome,
    /// The modules that failed.
    pub failures: Vec<Failure>,
}

/// Dispatch `event` to the subscribers of `kind` among `modules` whose
/// subscription `follows`, at once.
pub async fn dispatch(
    modules: &[Arc<Guarded>],
    scope: &Scope,
    kind: EventKind,
    event: &Event,
    follows: impl Fn(&Subscription) -> bool,
) -> Dispatched {
    let subscribed = modules.iter().filter_map(|guarded| {
        guarded
            .module()
            .manifest()
            .events
            .iter()
            .find(|s| s.kind == i32::from(kind) && follows(s))
            .map(|subscription| (guarded, subscription.clone()))
    });
    let answers = join_all(subscribed.map(|(guarded, subscription)| async move {
        let deadline = scope.deadline.min(Instant::now() + allowed(&subscription));
        // A module without conversation access gets the event without it;
        // a memory write names what changed, a module-available event the
        // modules, never what was said.
        let event = if matches!(kind, EventKind::MemoryWrite | EventKind::ModuleAvailable)
            || grant::reads_conversation(guarded.module().manifest())
        {
            event.clone()
        } else {
            grant::without_conversation(event)
        };
        let answer = guarded
            .run(&scope.cancel, deadline, "event", |m| {
                let scope = scope.clone();
                Box::pin(async move { m.on_event(&scope, &event).await })
            })
            .await;
        (guarded.id(), fail_closed(&subscription), answer)
    }))
    .await;
    merge(&event.hook_event_name, answers)
}

/// How long a subscription allows.
fn allowed(subscription: &Subscription) -> Duration {
    match subscription.deadline_ms {
        0 => DEFAULT_EVENT_DEADLINE,
        ms => Duration::from_millis(u64::from(ms)),
    }
}

/// Whether the subscription is fail-closed (unset is fail-open).
fn fail_closed(subscription: &Subscription) -> bool {
    FailurePolicy::try_from(subscription.policy) == Ok(FailurePolicy::FailClosed)
}

/// Merge the answers, in load order. Any module may block by its answer;
/// the failure policy decides only what a failure does (owner 2026-11-18:
/// memory, fail-open, holds a file's first edit once).
fn merge(
    hook_event_name: &str,
    answers: Vec<(&str, bool, Result<Outcome, ModuleError>)>,
) -> Dispatched {
    let mut blocked = false;
    let mut reasons = Vec::new();
    let mut context = Vec::new();
    let mut failures = Vec::new();
    for (module, closed, answer) in answers {
        match answer {
            Ok(outcome) => {
                if outcome.decision == BLOCK {
                    blocked = true;
                    reasons.push(outcome.reason);
                }
                if let Some(added) = outcome.hook_specific_output
                    && !added.additional_context.is_empty()
                {
                    context.push(added.additional_context);
                }
            }
            Err(error) => {
                if closed {
                    blocked = true;
                    reasons.push(format!("module {module} failed: {}", error.message));
                }
                failures.push(Failure {
                    module: module.to_string(),
                    error,
                    blocked: closed,
                });
            }
        }
    }
    let hook_specific_output = (!context.is_empty()).then(|| HookSpecificOutput {
        hook_event_name: hook_event_name.to_string(),
        additional_context: context.join("\n\n"),
    });
    Dispatched {
        outcome: Outcome {
            decision: if blocked {
                BLOCK.to_string()
            } else {
                String::new()
            },
            reason: reasons.join("\n"),
            hook_specific_output,
        },
        failures,
    }
}
