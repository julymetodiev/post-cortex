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

/// A workspace whose session solved a problem in `src/a.rs`.
async fn workspace_knowing_a_file(memory: &Memory) -> Uuid {
    let ws = memory
        .workspaces
        .create(CreateWorkspace {
            name: "p".into(),
            description: String::new(),
        })
        .await
        .unwrap();
    let sid = memory.sessions.create(None, None).await.unwrap();
    memory
        .workspaces
        .add_session(
            ws.id,
            sid,
            post_cortex_core::workspace::SessionRole::Primary,
        )
        .await
        .unwrap();
    let mut req = crate::capabilities::records::good_request();
    req.interaction_type = post_cortex_core::core::context_update::UpdateType::ProblemSolved;
    req.session_id = sid;
    req.content.title = "Scope lost its tenant".into();
    req.content.description = "own() must keep the tenant".into();
    req.code_reference = Some(post_cortex_core::core::context_update::CodeReference {
        file_path: "src/a.rs".into(),
        start_line: 0,
        end_line: 0,
        code_snippet: String::new(),
        commit_hash: None,
        branch: None,
        change_description: String::new(),
    });
    memory.records.write(req).await.unwrap();
    ws.id
}

fn on_file(name: &str, session: &str) -> Event {
    event(json!({
        "hook_event_name": name, "session_id": session, "tool_name": "Edit",
        "tool_input": {"file_path": "src/a.rs"}
    }))
}

/// The first edit of a file the memory knows is held once with its memory
/// as the reason; the same session's next edit goes through; another
/// session is held again.
#[tokio::test]
async fn a_file_s_first_edit_is_held_once_per_session() {
    let (memory, _dir) = crate::capabilities::test_support::rocksdb_memory().await;
    let ws = workspace_knowing_a_file(&memory).await;
    let scope = scope(Some(ws), None);

    let held = answer(&memory, &scope, &on_file("PreToolUse", "agent-1"))
        .await
        .unwrap();
    assert_eq!(held.decision, "block");
    assert!(held.reason.starts_with("post-cortex memory for src/a.rs:"));
    assert!(held.reason.contains("Scope lost its tenant"));
    assert!(held.reason.ends_with(HELD));
    assert!(held.hook_specific_output.is_none());

    let again = answer(&memory, &scope, &on_file("PreToolUse", "agent-1"))
        .await
        .unwrap();
    assert_eq!(again, Outcome::default());

    let other = answer(&memory, &scope, &on_file("PreToolUse", "agent-2"))
        .await
        .unwrap();
    assert_eq!(other.decision, "block");
}

/// A read tells the file's memory, and the edit after it goes through.
#[tokio::test]
async fn a_read_tells_the_file_and_its_edit_goes_through() {
    let (memory, _dir) = crate::capabilities::test_support::rocksdb_memory().await;
    let ws = workspace_knowing_a_file(&memory).await;
    let scope = scope(Some(ws), None);

    let read = answer(&memory, &scope, &on_file("PostToolUse", "agent-1"))
        .await
        .unwrap();
    assert_eq!(read.decision, "");
    assert!(added(&read).contains("Scope lost its tenant"));
    assert_eq!(
        read.hook_specific_output.unwrap().hook_event_name,
        "PostToolUse"
    );

    for name in ["PreToolUse", "PostToolUse"] {
        let after = answer(&memory, &scope, &on_file(name, "agent-1"))
            .await
            .unwrap();
        assert_eq!(after, Outcome::default(), "{name}");
    }
}

/// A client that names no session is told on every edit, never held.
#[tokio::test]
async fn without_a_session_the_memory_is_added_every_time() {
    let (memory, _dir) = crate::capabilities::test_support::rocksdb_memory().await;
    let ws = workspace_knowing_a_file(&memory).await;
    for _ in 0..2 {
        let outcome = answer(&memory, &scope(Some(ws), None), &on_file("PreToolUse", ""))
            .await
            .unwrap();
        assert_eq!(outcome.decision, "");
        assert!(added(&outcome).contains("Scope lost its tenant"));
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

/// `entity` depends on `via` through `relation`, one hop away.
fn dependent(entity: &str, via: &str, relation: RelationType) -> ImpactItem {
    ImpactItem {
        entity: entity.into(),
        entity_type: Some("code_component".into()),
        via: via.into(),
        relation_type: relation.as_str().into(),
        context: String::new(),
        depth: 1,
    }
}

/// A code component named `name`.
fn node(name: &str) -> crate::graph_views::GraphNode {
    let at = chrono::Utc::now();
    crate::graph_views::GraphNode {
        name: name.into(),
        entity_type: "code_component".into(),
        aliases: Vec::new(),
        description: None,
        importance: 1.0,
        mention_count: 1,
        records: 1,
        first_seen: at,
        last_seen: at,
        community: None,
    }
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
        dependents: vec![
            dependent(
                "compose_runner_tests",
                "Scope::working_in",
                RelationType::DependsOn,
            ),
            dependent(
                "ServiceRoute::target",
                "Scope.tenant",
                RelationType::Implements,
            ),
            dependent("Scope.tenant", "OrgStore::slug", RelationType::RequiredBy),
        ],
        changed_with: vec![node("pcx-host route"), node("SharedDocuments")],
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
             - 2026-10-01 Use one lock: {description} (no longer holds: superseded) [entry {nil}]\n\n\
             Depends on it:\n\
             - compose_runner_tests (depends on Scope::working_in)\n\
             - ServiceRoute::target (implements Scope.tenant)\n\
             - Scope.tenant (requires OrgStore::slug)\n\n\
             Changed with it: pcx-host route, SharedDocuments",
            nil = Uuid::nil()
        )
    );
    let empty = FileView {
        problems: Vec::new(),
        decisions: Vec::new(),
        dependents: Vec::new(),
        changed_with: Vec::new(),
        ..view
    };
    assert_eq!(render_file(&empty, "src/a.rs"), None);
}
