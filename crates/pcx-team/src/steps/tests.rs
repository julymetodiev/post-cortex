// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

use pcx_module_api::model::{StopReason, Usage};
use serde_json::json;

fn call(id: &str, name: &str, input: Value) -> TurnEvent {
    TurnEvent::ToolCall {
        id: id.to_string(),
        name: name.to_string(),
        input,
    }
}

fn result(id: &str, content: &str, is_error: bool) -> TurnEvent {
    TurnEvent::ToolResult {
        id: id.to_string(),
        content: content.to_string(),
        is_error,
    }
}

fn end(stop: TurnStop, steps: u32, answer: &str) -> TurnEnd {
    TurnEnd {
        stop,
        steps,
        usage: Usage::default(),
        answer: answer.to_string(),
    }
}

#[test]
fn the_last_calls_are_kept_and_a_failed_one_says_why() {
    let mut steps = Steps::default();
    for n in 0..7 {
        steps.see(&call(
            &format!("c{n}"),
            "bash",
            json!({"command": format!("ls {n}")}),
        ));
        steps.see(&result(&format!("c{n}"), "ok", false));
    }
    steps.see(&call("w", "write", json!({"path": "/tmp/x"})));
    steps.see(&TurnEvent::Text("thinking aloud".into()));
    steps.see(&result("w", "Permission\n denied `x`", true));
    let last = steps.last();
    assert_eq!(last.len(), SHOWN);
    assert_eq!(last[0], r#"→ bash {"command":"ls 3"}"#);
    assert_eq!(
        last[SHOWN - 1],
        r#"→ write {"path":"/tmp/x"} ✗ Permission denied 'x'"#
    );
    let long = "a".repeat(WIDTH + 10);
    assert_eq!(short(&long).chars().count(), WIDTH + 1);
}

#[test]
fn a_turn_at_its_most_steps_or_with_no_text_is_unanswered() {
    assert!(unanswered(&end(TurnStop::StepLimit, 50, "")));
    assert!(unanswered(&end(
        TurnStop::Answered(StopReason::EndTurn),
        3,
        "  "
    )));
    assert!(!unanswered(&end(
        TurnStop::Answered(StopReason::EndTurn),
        3,
        "done"
    )));
    // A failed or cancelled turn is tried again, then blocked: not this.
    assert!(!unanswered(&end(TurnStop::Cancelled, 9, "")));
}

#[test]
fn the_comment_says_how_it_ended_its_last_steps_and_its_session() {
    let comment = said(
        "dev",
        &end(TurnStop::StepLimit, 50, ""),
        &["→ bash {}".to_string()],
        "s-1",
    );
    assert!(comment.starts_with("dev stopped after 50 steps, the most a turn may take"));
    assert!(comment.contains("- `→ bash {}`"));
    assert!(comment.contains("session `s-1`"));
    assert!(comment.ends_with("Answer this comment for dev to go on."));
    let quiet = said(
        "dev",
        &end(TurnStop::Answered(StopReason::EndTurn), 2, ""),
        &[],
        "s-2",
    );
    assert!(quiet.starts_with("dev's turn ended (end_turn) after 2 steps"));
    assert!(quiet.contains("It called no tool."));
}
