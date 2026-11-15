// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use pcx_module_api::EventKind;
use tokio_util::sync::CancellationToken;

use crate::Host;

/// An executable script with `body` in `dir`, and its spec answering
/// `event` under `policy`.
fn script(
    dir: &tempfile::TempDir,
    name: &str,
    body: &str,
    event: &str,
    policy: ScriptPolicy,
) -> ScriptSpec {
    let path = dir.path().join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    ScriptSpec {
        id: name.to_string(),
        sha256: pin::sha256(&std::fs::read(&path).unwrap()),
        command: path,
        args: Vec::new(),
        events: vec![ScriptEvent {
            event: event.to_string(),
            policy,
            deadline_ms: 2_000,
        }],
        capabilities: vec![Grant::ReadConversation],
    }
}

async fn host(specs: Vec<ScriptSpec>) -> Host {
    let modules = specs
        .into_iter()
        .map(|s| Arc::new(ScriptModule::load(s).unwrap()) as Arc<dyn Module>)
        .collect();
    Host::start(modules, &HashMap::new(), &HashMap::new())
        .await
        .unwrap()
}

fn scope() -> Scope {
    Scope {
        tenant: String::new(),
        profile: String::new(),
        session_id: Some("s-1".to_string()),
        workspace_id: None,
        author: None,
        caller_module: None,
        call_depth: 0,
        turn: None,
        reach: None,
        role: None,
        read_only: false,
        person: None,
        deadline: Instant::now() + Duration::from_secs(10),
        cancel: CancellationToken::new(),
    }
}

fn event(name: &str) -> Event {
    serde_json::from_value(serde_json::json!({
        "hook_event_name": name,
        "tool_name": "Bash",
        "tool_input": {"command": "rm -rf /"}
    }))
    .unwrap()
}

#[test]
fn a_script_that_is_not_the_approved_one_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let mut spec = script(&dir, "a", "exit 0", "PreToolUse", ScriptPolicy::FailOpen);
    spec.sha256 = "00".repeat(32);
    assert!(matches!(
        ScriptModule::load(spec),
        Err(HostError::ExecutableNotApproved { .. })
    ));
    let unknown = script(&dir, "b", "exit 0", "Notification", ScriptPolicy::FailOpen);
    assert!(matches!(
        ScriptModule::load(unknown),
        Err(HostError::InvalidManifest { .. })
    ));
}

#[tokio::test]
async fn a_script_reads_the_event_and_answers_with_json() {
    let dir = tempfile::tempdir().unwrap();
    let seen = dir.path().join("seen.json");
    let body = format!(
        "cat > {}\necho '{{\"hookSpecificOutput\":{{\"hookEventName\":\"PreToolUse\",\"additionalContext\":\"careful\"}}}}'",
        seen.display()
    );
    let host = host(vec![script(
        &dir,
        "a",
        &body,
        "PreToolUse",
        ScriptPolicy::FailOpen,
    )])
    .await;
    let dispatched = host
        .dispatch(&scope(), EventKind::PreTool, &event("PreToolUse"))
        .await;
    assert!(dispatched.failures.is_empty(), "{:?}", dispatched.failures);
    let added = dispatched.outcome.hook_specific_output.unwrap();
    assert_eq!(added.additional_context, "careful");
    let input: serde_json::Value = serde_json::from_slice(&std::fs::read(&seen).unwrap()).unwrap();
    assert_eq!(input["tool_input"]["command"], "rm -rf /");
}

#[tokio::test]
async fn exit_two_blocks_with_the_reason() {
    let dir = tempfile::tempdir().unwrap();
    let body = "cat >/dev/null\necho 'no deleting the disk' >&2\nexit 2";
    let host = host(vec![script(
        &dir,
        "guard",
        body,
        "PreToolUse",
        ScriptPolicy::FailClosed,
    )])
    .await;
    let dispatched = host
        .dispatch(&scope(), EventKind::PreTool, &event("PreToolUse"))
        .await;
    assert_eq!(dispatched.outcome.decision, "block");
    assert_eq!(dispatched.outcome.reason, "no deleting the disk");
}

#[tokio::test]
async fn text_on_session_start_is_context_with_the_scope_in_the_environment() {
    let dir = tempfile::tempdir().unwrap();
    let body = "cat >/dev/null\necho \"session $PCX_SESSION_ID\"";
    let host = host(vec![script(
        &dir,
        "a",
        body,
        "SessionStart",
        ScriptPolicy::FailOpen,
    )])
    .await;
    let dispatched = host
        .dispatch(&scope(), EventKind::SessionStart, &event("SessionStart"))
        .await;
    let added = dispatched.outcome.hook_specific_output.unwrap();
    assert_eq!(added.additional_context, "session s-1");
}

#[tokio::test]
async fn a_script_changed_after_approval_is_refused_at_its_run() {
    let dir = tempfile::tempdir().unwrap();
    let spec = script(&dir, "a", "echo hi", "SessionStart", ScriptPolicy::FailOpen);
    let path = spec.command.clone();
    let host = host(vec![spec]).await;
    std::fs::write(&path, "#!/bin/sh\necho changed\n").unwrap();
    let dispatched = host
        .dispatch(&scope(), EventKind::SessionStart, &event("SessionStart"))
        .await;
    assert_eq!(dispatched.outcome, Outcome::default());
    assert_eq!(dispatched.failures[0].error.kind, ModuleErrorKind::Denied);
}

#[tokio::test]
async fn a_hung_script_is_killed_at_its_deadline() {
    let dir = tempfile::tempdir().unwrap();
    let mut spec = script(
        &dir,
        "a",
        "sleep 30",
        "SessionStart",
        ScriptPolicy::FailOpen,
    );
    spec.events[0].deadline_ms = 200;
    let host = host(vec![spec]).await;
    let started = Instant::now();
    let dispatched = host
        .dispatch(&scope(), EventKind::SessionStart, &event("SessionStart"))
        .await;
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(
        dispatched.failures[0].error.kind,
        ModuleErrorKind::DeadlineExceeded
    );
}

#[tokio::test]
async fn a_failing_script_fails_open() {
    let dir = tempfile::tempdir().unwrap();
    let host = host(vec![script(
        &dir,
        "a",
        "cat >/dev/null\nexit 1",
        "Stop",
        ScriptPolicy::FailOpen,
    )])
    .await;
    let dispatched = host
        .dispatch(&scope(), EventKind::TurnEnd, &event("Stop"))
        .await;
    assert_eq!(dispatched.outcome, Outcome::default());
    assert_eq!(dispatched.failures.len(), 1);
}

#[test]
fn a_spec_reads_from_toml_strictly() {
    let spec: ScriptSpec = toml::from_str(
        r#"
        id = "guard"
        command = "/opt/guard.sh"
        sha256 = "ab"
        events = [{ event = "PreToolUse", policy = "fail-closed", deadline_ms = 2000 }]
        "#,
    )
    .unwrap();
    assert!(matches!(spec.events[0].policy, ScriptPolicy::FailClosed));
    let typo: Result<ScriptSpec, _> = toml::from_str(
        r#"
        id = "guard"
        command = "/opt/guard.sh"
        sha256 = "ab"
        events = [{ event = "PreToolUse", polcy = "fail-closed" }]
        "#,
    );
    assert!(typo.is_err());
}

#[tokio::test]
async fn a_script_without_conversation_access_gets_no_conversation() {
    let dir = tempfile::tempdir().unwrap();
    let seen = dir.path().join("seen.json");
    let mut spec = script(
        &dir,
        "a",
        &format!("cat > {}", seen.display()),
        "PreToolUse",
        ScriptPolicy::FailOpen,
    );
    spec.capabilities = Vec::new();
    let host = host(vec![spec]).await;
    host.dispatch(&scope(), EventKind::PreTool, &event("PreToolUse"))
        .await;
    let input: serde_json::Value = serde_json::from_slice(&std::fs::read(&seen).unwrap()).unwrap();
    assert_eq!(input["tool_name"], "Bash");
    assert!(input.get("tool_input").is_none(), "{input}");
}
