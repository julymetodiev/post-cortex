// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

use std::time::{Duration, Instant};

use post_cortex_core::core::message::{MessageCursor, MessageOrder};
use post_cortex_core::services::UpdateCardRequest;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::capabilities::CreateWorkspace;

/// An event as a client hook sends it (Claude Code's JSON).
fn event(body: serde_json::Value) -> Event {
    serde_json::from_value(body).unwrap()
}

fn scope(workspace: Option<Uuid>, session: Option<Uuid>) -> Scope {
    Scope {
        tenant: String::new(),
        profile: String::new(),
        session_id: session.map(|id| id.to_string()),
        workspace_id: workspace.map(|id| id.to_string()),
        author: None,
        caller_module: None,
        call_depth: 0,
        turn: None,
        reach: None,
        role: None,
        read_only: false,
        person: None,
        deadline: Instant::now() + Duration::from_secs(5),
        cancel: CancellationToken::new(),
    }
}

fn added(outcome: &Outcome) -> &str {
    &outcome
        .hook_specific_output
        .as_ref()
        .expect("added context")
        .additional_context
}

#[tokio::test]
async fn session_start_puts_the_cards_in_front() {
    let (memory, _dir) = crate::capabilities::test_support::rocksdb_memory().await;
    let ws = memory
        .workspaces
        .create(CreateWorkspace {
            name: "p".into(),
            description: String::new(),
        })
        .await
        .unwrap();
    let start = event(json!({"hook_event_name": "SessionStart", "source": "startup"}));

    let bare = answer(&memory, &scope(Some(ws.id), None), &start)
        .await
        .unwrap();
    assert!(added(&bare).starts_with("## post-cortex cards\n\nNo project card yet."));
    assert!(added(&bare).contains(&ws.id.to_string()));

    memory
        .cards
        .update(UpdateCardRequest {
            scope: CardScope::Workspace(ws.id),
            text: "Rust 2024\n".into(),
            expected_version: 0,
            author: None,
        })
        .await
        .unwrap();
    let carded = answer(&memory, &scope(Some(ws.id), None), &start)
        .await
        .unwrap();
    assert_eq!(
        added(&carded),
        "## post-cortex cards\n\n### Project card (v1)\nRust 2024"
    );
    let output = carded.hook_specific_output.unwrap();
    assert_eq!(output.hook_event_name, "SessionStart");
}

#[tokio::test]
async fn an_event_without_its_scope_is_refused() {
    let (memory, _dir) = crate::capabilities::test_support::rocksdb_memory().await;
    let start = event(json!({"hook_event_name": "SessionStart"}));
    let refused = answer(&memory, &scope(None, None), &start).await;
    assert_eq!(
        refused.unwrap_err().details.parameter.as_deref(),
        Some("workspace_id")
    );
    let unknown = answer(
        &memory,
        &scope(None, None),
        &event(json!({"hook_event_name": "Notification"})),
    )
    .await;
    assert!(unknown.is_err());
}

#[tokio::test]
async fn a_tool_without_a_file_or_an_unknown_file_adds_nothing() {
    let (memory, _dir) = crate::capabilities::test_support::rocksdb_memory().await;
    let ws = memory
        .workspaces
        .create(CreateWorkspace {
            name: "p".into(),
            description: String::new(),
        })
        .await
        .unwrap();
    let bash = event(json!({
        "hook_event_name": "PreToolUse", "tool_name": "Bash",
        "tool_input": {"command": "ls"}
    }));
    let edit = event(json!({
        "hook_event_name": "PreToolUse", "tool_name": "Edit",
        "tool_input": {"file_path": "/repo/src/unknown.rs"}
    }));
    for e in [bash, edit] {
        let outcome = answer(&memory, &scope(Some(ws.id), None), &e)
            .await
            .unwrap();
        assert_eq!(outcome, Outcome::default());
    }
}

#[tokio::test]
async fn turn_end_captures_the_turns_once() {
    let (memory, _dir) = crate::capabilities::test_support::rocksdb_memory().await;
    let sid = memory.sessions.create(None, None).await.unwrap();
    let stop = event(json!({
        "hook_event_name": "Stop",
        "payload": {"messages": [
            {"turn_id": "u1", "role": "user", "text": "hello", "author": "claude-code"},
            {"turn_id": "a1", "role": "assistant", "text": "hi", "tools": ["Edit"]}
        ]}
    }));
    for _ in 0..2 {
        let outcome = answer(&memory, &scope(None, Some(sid)), &stop)
            .await
            .unwrap();
        assert_eq!(outcome, Outcome::default());
    }
    let stored = memory
        .messages
        .page(sid, MessageCursor::From(0), 10, MessageOrder::Asc)
        .await
        .unwrap();
    let texts: Vec<&str> = stored.iter().map(|m| m.text.as_str()).collect();
    assert_eq!(texts, ["hello", "hi"]);
}

#[test]
fn a_file_s_memory_lists_problems_decisions_and_dependents() {
    let record = |title: &str, ended: bool| RecordView {
        id: Uuid::nil(),
        session_id: Uuid::nil(),
        interaction_type: "problem_solved".into(),
        title: title.into(),
        description: "x".repeat(300),
        author: None,
        timestamp: chrono::DateTime::parse_from_rfc3339("2026-10-01T10:00:00Z")
            .unwrap()
            .into(),
        confidence: None,
        derived_from: Vec::new(),
        file: None,
        validity: post_cortex_core::graph::temporal::Validity {
            valid_from: chrono::Utc::now(),
            valid_to: ended.then(chrono::Utc::now),
            ended_by: None,
            reason: ended.then_some(post_cortex_core::graph::temporal::EndReason::Superseded),
        },
    };
    let view = FileView {
        path: "src/a.rs".into(),
        records: Vec::new(),
        problems: vec![record("Deadlock", false)],
        decisions: vec![record("Use one lock", true)],
        entities: Vec::new(),
        dependents: Vec::new(),
    };
    let text = render_file(&view, "src/a.rs").unwrap();
    let description = "x".repeat(DESCRIPTION_CHARS);
    assert_eq!(
        text,
        format!(
            "post-cortex memory for src/a.rs:\n\n\
             Problems solved here before:\n\
             - 2026-10-01 Deadlock: {description} [entry {nil}]\n\n\
             Decisions and requirements about it:\n\
             - 2026-10-01 Use one lock: {description} (no longer holds: superseded) [entry {nil}]",
            nil = Uuid::nil()
        )
    );
    let empty = FileView {
        problems: Vec::new(),
        decisions: Vec::new(),
        ..view
    };
    assert_eq!(render_file(&empty, "src/a.rs"), None);
}
