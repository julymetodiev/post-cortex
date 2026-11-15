// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A step's failures (docs/modules/workflow.md, errors): their kinds, a
//! module's as the engine names them and the engine's own, which of them
//! a block tries again, and how long it waits before it does.

use chrono::TimeDelta;
use pcx_module_api::Effect;
use post_cortex_proto::workflow::Failure;

/// The first retry's wait; each next one doubles it.
const FIRST_BACKOFF: TimeDelta = TimeDelta::seconds(1);

/// The longest a retry waits.
const MAX_BACKOFF: TimeDelta = TimeDelta::hours(1);

/// A failure's kinds of the engine's own, beside a module's
/// (`invalid_argument`, `unavailable`, ...).
pub mod kinds {
    /// An attempt ran past its block's timeout.
    pub const TIMEOUT: &str = "timeout";
    /// A person declined the decision an approval waited on.
    pub const DECLINED: &str = "declined";
    /// A template did not fill (a value out of a field's range).
    pub const TEMPLATE: &str = "template";
    /// An expression failed or spent its budget.
    pub const EXPRESSION: &str = "expression";
    /// A model's or an agent's answer did not fit its schema.
    pub const ANSWER: &str = "answer";
    /// A budget of the run ran out.
    pub const BUDGET: &str = "budget";
    /// The run was cancelled.
    pub const CANCELLED: &str = "cancelled";
    /// A `validate` found violations and has no `invalid` edge.
    pub const INVALID: &str = "invalid";
}

/// Whether a failure of `kind` may be tried again: a read whatever
/// failed it but its own input; anything else only when the failure
/// passes (the module unavailable or offline, a deadline, a timeout, a
/// limit reset later), so a write that may have been done is not done
/// twice (an `external` tool carries its idempotency key for that).
pub fn retryable(effect: Option<Effect>, kind: &str) -> bool {
    let passing = matches!(
        kind,
        "unavailable" | "offline" | "deadline_exceeded" | "resource_exhausted" | kinds::TIMEOUT
    );
    let own_input = matches!(
        kind,
        kinds::TEMPLATE | kinds::EXPRESSION | kinds::BUDGET | kinds::CANCELLED | "invalid_argument"
    );
    match effect {
        Some(Effect::Read) => !own_input,
        _ => passing,
    }
}

/// How long the retry after attempt `attempt` (from 1) waits: 1 s,
/// doubling, at most an hour.
pub fn backoff(attempt: u32) -> TimeDelta {
    let doublings = attempt.saturating_sub(1).min(12);
    (FIRST_BACKOFF * (1 << doublings)).min(MAX_BACKOFF)
}

/// A failure of `kind`.
pub fn failure(kind: &str, message: &str) -> Failure {
    Failure {
        kind: kind.to_string(),
        message: message.to_string(),
    }
}
