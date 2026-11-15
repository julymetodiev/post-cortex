// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! An action that waits for a person: how long before it is said again
//! and given up, what it is owed for the time it has waited, why its
//! decision's end gives it up, and what its end tells who asked. Pure.

use chrono::{DateTime, Utc};
use post_cortex_proto::board::Decision;
use post_cortex_proto::ops::{Action, OpsConfig};

/// Minutes a waiting action is said again after when the settings name none.
const REMIND: u32 = 30;

/// Minutes a waiting action is given up after when the settings name none.
const GIVE_UP: u32 = 240;

/// Why the action that waits on `decision` will not run: a person declined
/// it, or who asked it withdrew it; none while it may still be approved.
#[must_use]
pub fn given_up(decision: &Decision) -> Option<String> {
    if let Some(person) = &decision.declined_by {
        return Some(match &decision.decline_note {
            Some(note) => format!("declined by {person} ({note})"),
            None => format!("declined by {person}"),
        });
    }
    decision
        .withdrawn
        .as_ref()
        .map(|why| format!("its decision was withdrawn ({why})"))
}

/// The minutes an action waits for a person before it is said again and
/// before it is given up, as `config` sets them (30 and 240 unless it says).
#[must_use]
pub fn patience(config: &OpsConfig) -> (u32, u32) {
    let or = |minutes: u32, default: u32| if minutes == 0 { default } else { minutes };
    (
        or(config.remind_minutes, REMIND),
        or(config.give_up_minutes, GIVE_UP),
    )
}

/// What a waiting action is owed for the time it has waited.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Waited {
    /// Nothing yet.
    Short,
    /// The channel is told once that it still waits: what to say.
    Remind(String),
    /// It is given up: why its decision is withdrawn.
    GiveUp(String),
}

/// What the waiting `action` is owed at `now`: given up when no one
/// answered within the settings' minutes (240), said again once after the
/// reminder's (30), else nothing. One that says no time it was asked is
/// owed nothing.
#[must_use]
pub fn waited(config: &OpsConfig, action: &Action, now: DateTime<Utc>) -> Waited {
    let asked = action
        .asked
        .as_ref()
        .and_then(|t| DateTime::from_timestamp(t.seconds, 0));
    let Some(asked) = asked else {
        return Waited::Short;
    };
    let minutes = (now - asked).num_minutes();
    let (remind, give_up) = patience(config);
    if minutes >= i64::from(give_up) {
        return Waited::GiveUp(format!("no one answered in {give_up} minutes"));
    }
    if action.reminded.is_none() && minutes >= i64::from(remind) {
        return Waited::Remind(format!(
            "Still waits for a person, {minutes} minutes now: {}. Approve or decline it; unanswered {give_up} minutes after it was asked, it is given up.",
            action.what
        ));
    }
    Waited::Short
}

/// What an action's end tells `asked_by` when it ran on a person's
/// approval, after the asker's turn: the mention wakes a member of the
/// team to check what the action did.
#[must_use]
pub fn look_again(asked_by: &str) -> String {
    format!(
        "@{asked_by}: this ended after your turn, on a person's approval. Look at how its target is now and say on the task whether it helped."
    )
}
