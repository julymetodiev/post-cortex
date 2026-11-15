// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Golden test: the JSON form of `pcx.module.v1` hook messages is the hook
//! JSON of Claude Code and Codex (ARCHITECTURE section 7).

use post_cortex_proto::module::{Event, HookSpecificOutput, Outcome};
use serde_json::{Value, json};

/// A Claude Code PreToolUse payload as the client sends it.
fn claude_pre_tool() -> Value {
    json!({
        "session_id": "abc123",
        "transcript_path": "/Users/me/.claude/projects/p/abc123.jsonl",
        "cwd": "/repo",
        "permission_mode": "default",
        "hook_event_name": "PreToolUse",
        "tool_use_id": "toolu_01",
        "tool_name": "Edit",
        "tool_input": { "file_path": "/repo/src/main.rs", "old_string": "a", "new_string": "b" }
    })
}

#[test]
fn claude_code_events_read_unchanged() {
    let event: Event = serde_json::from_value(claude_pre_tool()).unwrap();
    assert_eq!(event.hook_event_name, "PreToolUse");
    assert_eq!(event.session_id, "abc123");
    assert_eq!(event.cwd, "/repo");
    assert_eq!(event.tool_name, "Edit");
    let input = serde_json::to_value(event.tool_input.unwrap()).unwrap();
    assert_eq!(input["file_path"], "/repo/src/main.rs");
}

#[test]
fn codex_turn_end_reads_unchanged() {
    let event: Event = serde_json::from_value(json!({
        "session_id": "c1",
        "turn_id": "t7",
        "hook_event_name": "Stop",
        "last_assistant_message": "done"
    }))
    .unwrap();
    assert_eq!(
        (
            event.turn_id.as_str(),
            event.last_assistant_message.as_str()
        ),
        ("t7", "done")
    );
}

#[test]
fn outcomes_are_claude_code_answers() {
    let outcome = Outcome {
        decision: String::new(),
        reason: String::new(),
        hook_specific_output: Some(HookSpecificOutput {
            hook_event_name: "UserPromptSubmit".into(),
            additional_context: "Relevant memory: ...".into(),
        }),
    };
    assert_eq!(
        serde_json::to_value(outcome).unwrap(),
        json!({
            "hookSpecificOutput": {
                "hookEventName": "UserPromptSubmit",
                "additionalContext": "Relevant memory: ..."
            }
        })
    );
    let block = Outcome {
        decision: "block".into(),
        reason: "secret in the command".into(),
        hook_specific_output: None,
    };
    assert_eq!(
        serde_json::to_value(block).unwrap(),
        json!({ "decision": "block", "reason": "secret in the command" })
    );
}
