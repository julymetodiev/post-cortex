// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Asking a person to approve a module's code or grants on the board
//! (docs/modules/workflow.md, triggers and approval; docs/modules/dynamic.md,
//! approved): the board's requests and how its answers read, the one place a
//! decision of risk `code` is made. Pure: a module sends them over its own
//! channel, in its own name, and keeps what was approved.

use post_cortex_proto::board::{
    AddTaskRequest, DecideRequest, DecisionApproved, DecisionDeclined, DecisionRecord, Risk,
    Thread, WithdrawDecisionRequest,
};
use post_cortex_proto::module::BusEvent;
use prost::{Message, Name};

/// The board's RPC that adds a task.
pub const ADD_TASK: &str = "/pcx.board.v1.Boards/AddTask";
/// The board's RPC that records a decision.
pub const DECIDE: &str = "/pcx.board.v1.Boards/Decide";
/// The board's RPC that reads a task with its thread.
pub const GET_TASK: &str = "/pcx.board.v1.Boards/GetTask";
/// The board's RPC by which a decision's author withdraws it.
pub const WITHDRAW: &str = "/pcx.board.v1.Boards/WithdrawDecision";
/// The board's event of a decision approved.
pub const DECISION_APPROVED: &str = "board.decision_approved";
/// The board's event of a decision a person declined.
pub const DECISION_DECLINED: &str = "board.decision_declined";

/// Where a module asks: its settings' workspace and board.
#[derive(Debug, Clone, Copy)]
pub struct Place<'a> {
    /// The workspace.
    pub workspace: &'a str,
    /// The board.
    pub board: &'a str,
}

/// The task a question is asked on: added once, by `key`.
#[must_use]
pub fn task(place: Place<'_>, title: String, text: String, key: String) -> AddTaskRequest {
    AddTaskRequest {
        workspace_id: place.workspace.to_string(),
        board: place.board.to_string(),
        title,
        text,
        key: Some(key),
        ..Default::default()
    }
}

/// The question `what` on task `task`, of risk `code`: it holds the task
/// until a person, or a team's policy that allows it, approves.
#[must_use]
pub fn question(place: Place<'_>, task: String, what: String, why: String) -> DecideRequest {
    DecideRequest {
        workspace_id: place.workspace.to_string(),
        task,
        what,
        why,
        risk: Risk::Code.into(),
        ..Default::default()
    }
}

/// Decision `decision` in `workspace` withdrawn by its author because of
/// `why`: no one is asked any more.
#[must_use]
pub fn withdrawal(workspace: &str, decision: String, why: String) -> WithdrawDecisionRequest {
    WithdrawDecisionRequest {
        workspace_id: workspace.to_string(),
        decision,
        why,
    }
}

/// Who approved the decision at once, when a team's policy did.
#[must_use]
pub fn approved_at_once(decided: &DecisionRecord) -> Option<String> {
    decided
        .decision
        .as_ref()
        .and_then(|d| d.approved_by.clone())
}

/// Decision `decision` of `thread`, as the board's event says it, when it
/// was approved: read again when a module starts, as the event may have
/// passed while it did not run.
#[must_use]
pub fn approved_in(thread: &Thread, task: &str, decision: &str) -> Option<DecisionApproved> {
    thread
        .decisions
        .iter()
        .filter(|d| d.id == decision)
        .find_map(|d| d.decision.as_ref()?.approved_by.clone())
        .map(|approved_by| DecisionApproved {
            decision: decision.to_string(),
            task: task.to_string(),
            approved_by,
        })
}

/// Decision `decision` of `thread`, as the board's event says it, when a
/// person declined it: read again when a module starts, as [`approved_in`].
#[must_use]
pub fn declined_in(thread: &Thread, task: &str, decision: &str) -> Option<DecisionDeclined> {
    thread
        .decisions
        .iter()
        .filter(|d| d.id == decision)
        .find_map(|d| d.decision.as_ref()?.declined_by.clone())
        .map(|declined_by| DecisionDeclined {
            decision: decision.to_string(),
            task: task.to_string(),
            declined_by,
        })
}

/// The message of `event` when it is named `name` and carries a `T`.
fn said<T: Message + Name + Default>(event: &BusEvent, name: &str) -> Option<T> {
    if event.name != name {
        return None;
    }
    let payload = event.payload.as_ref()?;
    // A type's URL is its name after the last `/`, whatever comes before it.
    if payload.type_url.rsplit('/').next() != Some(T::full_name().as_str()) {
        return None;
    }
    match T::decode(payload.value.as_ref()) {
        Ok(said) => Some(said),
        Err(e) => {
            tracing::warn!(event = %event.id, "a {name} does not read: {e}");
            None
        }
    }
}

/// What `event` says, when it is a decision approved.
#[must_use]
pub fn heard(event: &BusEvent) -> Option<DecisionApproved> {
    said(event, DECISION_APPROVED)
}

/// What `event` says, when it is a decision a person declined.
#[must_use]
pub fn declined(event: &BusEvent) -> Option<DecisionDeclined> {
    said(event, DECISION_DECLINED)
}

#[cfg(test)]
mod tests {
    use super::*;
    use post_cortex_proto::board::Decision;

    const PLACE: Place<'static> = Place {
        workspace: "w",
        board: "b",
    };

    fn record(id: &str, approved_by: Option<&str>) -> DecisionRecord {
        DecisionRecord {
            id: id.to_string(),
            decision: Some(Decision {
                approved_by: approved_by.map(str::to_string),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn a_question_is_of_risk_code_on_its_place() {
        let asked = question(PLACE, "t".into(), "what".into(), "why".into());
        assert_eq!(asked.risk, i32::from(Risk::Code));
        assert_eq!(asked.workspace_id, "w");
        let added = task(PLACE, "title".into(), "text".into(), "key".into());
        assert_eq!(added.board, "b");
        assert_eq!(added.key.as_deref(), Some("key"));
    }

    #[test]
    fn only_the_decision_asked_and_approved_reads_as_approved() {
        let thread = Thread {
            decisions: vec![record("other", Some("ana")), record("d", None)],
            ..Default::default()
        };
        assert!(approved_in(&thread, "t", "d").is_none());
        let thread = Thread {
            decisions: vec![record("other", None), record("d", Some("ana"))],
            ..Default::default()
        };
        let approved = approved_in(&thread, "t", "d").unwrap();
        assert_eq!(approved.approved_by, "ana");
        assert_eq!(approved.task, "t");
        assert_eq!(
            approved_at_once(&record("d", Some("policy"))).as_deref(),
            Some("policy")
        );
        assert!(approved_at_once(&record("d", None)).is_none());
    }

    #[test]
    fn an_event_of_another_name_or_type_is_not_heard() {
        let approved = DecisionApproved {
            decision: "d".into(),
            task: "t".into(),
            approved_by: "ana".into(),
        };
        let payload = |type_url: String| pbjson_types::Any {
            type_url,
            value: approved.encode_to_vec().into(),
        };
        let event = BusEvent {
            name: DECISION_APPROVED.to_string(),
            // As the board writes it.
            payload: Some(payload(format!(
                "type.googleapis.com/{}",
                DecisionApproved::full_name()
            ))),
            ..Default::default()
        };
        assert_eq!(heard(&event).unwrap().decision, "d");
        let other_name = BusEvent {
            name: "board.task_moved".to_string(),
            ..event.clone()
        };
        assert!(heard(&other_name).is_none());
        let other_type = BusEvent {
            payload: Some(payload("type.googleapis.com/other.Type".to_string())),
            ..event
        };
        assert!(heard(&other_type).is_none());
    }

    #[test]
    fn a_decision_a_person_declined_reads_as_declined_and_not_as_approved() {
        let thread = Thread {
            decisions: vec![DecisionRecord {
                id: "d".into(),
                decision: Some(Decision {
                    declined_by: Some("ana".into()),
                    ..Default::default()
                }),
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(approved_in(&thread, "t", "d").is_none());
        let said = declined_in(&thread, "t", "d").unwrap();
        assert_eq!(
            (said.declined_by.as_str(), said.task.as_str()),
            ("ana", "t")
        );
        let event = BusEvent {
            name: DECISION_DECLINED.to_string(),
            payload: Some(pbjson_types::Any {
                type_url: format!("type.googleapis.com/{}", DecisionDeclined::full_name()),
                value: said.encode_to_vec().into(),
            }),
            ..Default::default()
        };
        assert_eq!(declined(&event).unwrap().decision, "d");
        assert!(heard(&event).is_none());
    }
}
