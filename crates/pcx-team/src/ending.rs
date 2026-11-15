// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! How a member's work on a task ends (ARCHITECTURE section 8, operations,
//! W32): done, worth another try after a pause (it ran past its deadline,
//! its model failed for a while, a call failed for a while), or not (a
//! module blocked it, a call the rules refuse, its model's quota used up or
//! its credentials refused), waiting for a runner that is offline (no try spent:
//! docs/connect.md section 7), or cut short with its work unfinished, for
//! a person (no try spent: docs/modules/team.md, a turn without an answer);
//! and the
//! task blocked when the tries ran out. Pure.

use std::time::Duration;

use pcx_module_api::agent::{TurnEnd, TurnStop};
use pcx_module_api::model::{ModelFailure, ModelFailureKind, StopReason};
use pcx_module_api::{ModuleError, ModuleErrorKind};
use post_cortex_proto::team::TeamConfig;

/// The label of a task whose work failed its tries.
pub const BLOCKED: &str = "blocked";

/// The label of a task whose work waits for its runner to connect.
pub const WAITING: &str = "waiting-runner";

/// The label of a task whose member's turn was cut short with its work
/// unfinished: it waits for a person.
pub const WAITING_PERSON: &str = "waiting-person";

/// Minutes a turn may run when the settings name none.
const WORK_MINUTES: u32 = 30;

/// Tries when the settings name none.
const TRIES: u32 = 2;

/// The pause before another try, unless the provider names a longer one.
const PAUSE: Duration = Duration::from_secs(60);

/// How a try ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ending {
    /// The member answered: the work is done.
    Done,
    /// It was cut short with its work unfinished (its most steps, or its
    /// model's most tokens without text): it stops, no try spent, until a
    /// person answers.
    Halt,
    /// It failed in a way another try may not: why, and how long to wait
    /// before it.
    Retry {
        /// Why.
        why: String,
        /// The pause before another try.
        after: Duration,
    },
    /// It failed in a way another try would too: why.
    Stop(String),
    /// A module it needs runs on a runner that is offline: it goes on when
    /// the runner connects, no try spent; why.
    Wait(String),
}

/// How a turn that ended as `end` ended for its work.
#[must_use]
pub fn ended(end: &TurnEnd) -> Ending {
    match &end.stop {
        TurnStop::Answered(StopReason::MaxTokens) if end.answer.trim().is_empty() => Ending::Halt,
        TurnStop::Answered(_) => Ending::Done,
        TurnStop::StepLimit => Ending::Halt,
        TurnStop::Repeating(what) => Ending::Stop(format!("its tools kept failing: {what}")),
        TurnStop::Cancelled => retry("it ran past its deadline".to_string(), None),
        TurnStop::Failed(failure) => model_failed(failure),
        TurnStop::Blocked(reason) => Ending::Stop(format!("a module blocked it: {reason}")),
    }
}

/// How a turn that failed with `error` ended for its work.
#[must_use]
pub fn failed(error: &ModuleError) -> Ending {
    match error.kind {
        ModuleErrorKind::InvalidArgument
        | ModuleErrorKind::NotFound
        | ModuleErrorKind::Denied
        | ModuleErrorKind::FailedPrecondition => Ending::Stop(error.message.clone()),
        ModuleErrorKind::Offline => Ending::Wait(error.message.clone()),
        _ => retry(error.message.clone(), None),
    }
}

/// How a turn whose model failed past its recovery ended: another try only
/// for a failure that passes (rate limited, overloaded, unreachable).
fn model_failed(failure: &ModelFailure) -> Ending {
    let why = format!("its model failed: {failure}");
    match failure.kind {
        ModelFailureKind::RateLimited
        | ModelFailureKind::Overloaded
        | ModelFailureKind::Unavailable => retry(why, failure.retry_after),
        ModelFailureKind::QuotaExhausted
        | ModelFailureKind::Unauthorized
        | ModelFailureKind::ContextTooLong
        | ModelFailureKind::Invalid => Ending::Stop(why),
    }
}

/// Another try for `why`, after the pause or the provider's longer one.
fn retry(why: String, retry_after: Option<Duration>) -> Ending {
    Ending::Retry {
        why,
        after: retry_after.map_or(PAUSE, |after| after.max(PAUSE)),
    }
}

/// How long a member's turn may run, as `config` sets it (30 minutes).
#[must_use]
pub fn deadline(config: &TeamConfig) -> Duration {
    let minutes = match config.work_minutes {
        0 => WORK_MINUTES,
        m => m,
    };
    Duration::from_secs(u64::from(minutes) * 60)
}

/// How often a member's work is tried, as `config` sets it (2).
#[must_use]
pub fn tries(config: &TeamConfig) -> u32 {
    match config.tries {
        0 => TRIES,
        n => n,
    }
}

/// `labels` with `label`, once.
#[must_use]
pub fn with_label(labels: &[String], label: &str) -> Vec<String> {
    let mut labels = labels.to_vec();
    if !labels.iter().any(|l| l == label) {
        labels.push(label.to_string());
    }
    labels
}

/// `labels` without any of `off`.
#[must_use]
pub fn without_labels(labels: &[String], off: &[&str]) -> Vec<String> {
    labels
        .iter()
        .filter(|l| !off.contains(&l.as_str()))
        .cloned()
        .collect()
}

/// The comment on a task whose `member`'s work waits for its runner, which
/// is offline (`why`).
#[must_use]
pub fn waits(member: &str, why: &str) -> String {
    format!(
        "Waiting: {member}'s work needs the runner, which is offline ({why}). It goes on when the runner connects; nothing else is needed."
    )
}

/// The comment on a task whose `member`'s turn a person stopped.
#[must_use]
pub fn stopped(member: &str) -> String {
    format!("Stopped: a person stopped {member}'s turn. A new comment naming it wakes it again.")
}

/// The comment on a task whose `member`'s work failed `tries` tries,
/// the last for `why`.
#[must_use]
pub fn said(member: &str, tries: u32, why: &str) -> String {
    let times = if tries == 1 { "once" } else { "every time" };
    format!(
        "Blocked: {member}'s work failed {times} it was tried ({tries}); the last time {why}. A person looks at it, then asks again in a comment naming it: that takes the label off."
    )
}

#[cfg(test)]
mod tests;
