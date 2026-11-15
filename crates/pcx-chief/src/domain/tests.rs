// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use serde_json::json;

use super::*;

#[test]
fn a_conversation_goes_on_only_with_who_started_it_from_its_channel() {
    let talk = Talk {
        asked_by: "julius".to_string(),
        channel: "telegram".to_string(),
        session_id: "s1".to_string(),
        ..Talk::default()
    };
    assert_eq!(talk.continued_by("julius", "telegram"), Ok(()));
    assert_eq!(
        talk.continued_by("mallory", "telegram"),
        Err(Refused::NotYours)
    );
    assert_eq!(talk.continued_by("julius", ""), Err(Refused::NotYours));
}

#[test]
fn the_budget_counts_input_output_and_cache_writes() {
    let usage = Usage {
        input: 10,
        output: 5,
        cache_read: 1000,
        cache_write: 3,
    };
    assert_eq!(spent(&usage), 18);
    assert_eq!(within(17, 18), Ok(()));
    assert_eq!(
        within(18, 18),
        Err(Refused::Spent {
            used: 18,
            budget: 18
        })
    );
}

#[test]
fn a_call_is_recorded_when_its_result_comes_and_one_without_stays_open() {
    let mut calls = Calls::default();
    let call = |id: &str, name: &str| TurnEvent::ToolCall {
        id: id.to_string(),
        name: name.to_string(),
        input: json!({ "id": id }),
    };
    assert_eq!(calls.take(call("1", "task_add")), None);
    assert_eq!(calls.take(call("2", "comment")), None);
    assert_eq!(calls.take(TurnEvent::Text("thinking aloud".into())), None);
    let answered = calls.take(TurnEvent::ToolResult {
        id: "2".to_string(),
        content: "no".to_string(),
        is_error: true,
    });
    assert_eq!(
        answered,
        Some(Called {
            tool: "comment".to_string(),
            input: json!({ "id": "2" }),
            failed: Some(true),
        })
    );
    // A result of no open call is no action.
    let unknown = TurnEvent::ToolResult {
        id: "9".to_string(),
        content: String::new(),
        is_error: false,
    };
    assert_eq!(calls.take(unknown), None);
    let open = calls.unanswered();
    assert_eq!(open.len(), 1);
    let action = open[0].clone().action("talk-1", "julius").unwrap();
    assert_eq!(action.tool, "task_add");
    assert_eq!(action.failed, None);
    assert_eq!(action.talk, "talk-1");
    assert_eq!(
        serde_json::to_value(action.input.unwrap()).unwrap(),
        json!({ "id": "1" })
    );
}

#[test]
fn an_actions_input_is_the_object_the_model_gave() {
    let input = json!({ "task": "5f89e095", "members": ["a", "b"], "urgent": true });
    let called = |input| Called {
        tool: "comment".to_string(),
        input,
        failed: Some(false),
    };
    let action = called(input.clone()).action("talk-1", "julius").unwrap();
    assert_eq!(serde_json::to_value(action.input.unwrap()).unwrap(), input);
    assert!(
        called(json!("not an object"))
            .action("talk-1", "julius")
            .is_err()
    );
}
