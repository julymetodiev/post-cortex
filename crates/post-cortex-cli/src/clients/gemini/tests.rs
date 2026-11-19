// Copyright (c) 2026 Julius ML
// MIT License

use super::*;
use crate::clients::common::{cloud_args, remote};
use std::path::Path;

fn ctx(dir: &Path, capture: bool) -> SetupContext {
    SetupContext {
        project_dir: dir.to_path_buf(),
        session_id: "S1".into(),
        workspace_id: "W1".into(),
        daemon_url: "http://127.0.0.1:3737".into(),
        capture,
        profile: "self".into(),
        remote: false,
        global: false,
    }
}

fn offline() -> HookEnv {
    HookEnv::new(
        "http://127.0.0.1:9",
        reqwest::Client::new(),
        "S1",
        "W1",
        "gemini-cli",
    )
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn commands(settings: &Value, event: &str) -> Vec<String> {
    settings["hooks"][event]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|g| g["hooks"].as_array().cloned().unwrap_or_default())
        .filter_map(|h| h["command"].as_str().map(str::to_string))
        .collect()
}

#[test]
fn remote_install_writes_the_stdio_entry_and_no_token() {
    let dir = tempfile::tempdir().unwrap();
    install(&ctx(dir.path(), false)).unwrap();
    install(&remote(ctx(dir.path(), false))).unwrap();
    let text = std::fs::read_to_string(dir.path().join(SETTINGS)).unwrap();
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        v["mcpServers"]["post-cortex"],
        json!({ "command": "pcx", "args": cloud_args(Client::Gemini) })
    );
    assert!(
        !text.contains("PC_AUTH_TOKEN") && !text.contains("Bearer"),
        "{text}"
    );
    assert!(text.contains("--profile cloud"), "{text}");
}

#[test]
fn install_writes_settings_agents_and_skill() {
    let dir = tempfile::tempdir().unwrap();
    let lines = install(&ctx(dir.path(), false)).unwrap();
    assert_eq!(lines.len(), 3);
    assert!(lines[0].contains("Created: .gemini/settings.json"));

    let s = read_json(&dir.path().join(SETTINGS));
    let server = &s["mcpServers"]["post-cortex"];
    assert_eq!(server["httpUrl"], "http://127.0.0.1:3737/mcp");
    assert_eq!(server["headers"]["X-PCX-Author"], "gemini-cli");
    assert_eq!(
        server["headers"]["Authorization"],
        "Bearer ${PC_AUTH_TOKEN:-}"
    );
    assert_eq!(s["context"]["fileName"], json!(["AGENTS.md", "GEMINI.md"]));
    assert_eq!(
        commands(&s, "SessionStart"),
        vec![
            "pcx hook gemini session-start --session S1 --workspace W1 --profile self --url http://127.0.0.1:3737"
        ]
    );
    assert_eq!(s["hooks"]["AfterTool"][0]["matcher"], "read_file");
    assert_eq!(s["hooks"]["AfterTool"][0]["hooks"][0]["timeout"], 10_000);
    assert_eq!(s["hooks"]["BeforeTool"][0]["matcher"], "write_file|replace");
    assert_eq!(
        commands(&s, "BeforeTool"),
        vec![
            "pcx hook gemini pre-edit --session S1 --workspace W1 --profile self --url http://127.0.0.1:3737"
        ]
    );
    assert_eq!(commands(&s, "BeforeAgent").len(), 1);
    assert!(s["hooks"].get("AfterAgent").is_none());

    let agents = std::fs::read_to_string(dir.path().join("AGENTS.md")).unwrap();
    assert!(agents.contains("`S1`") && agents.contains("`W1`"));
    assert!(
        dir.path()
            .join(".agents/skills/post-cortex-memory/SKILL.md")
            .exists()
    );
}

#[test]
fn capture_adds_after_agent_and_turning_it_off_removes_it() {
    let dir = tempfile::tempdir().unwrap();
    install(&ctx(dir.path(), true)).unwrap();
    let s = read_json(&dir.path().join(SETTINGS));
    assert!(commands(&s, "AfterAgent")[0].starts_with("pcx hook gemini capture "));
    install(&ctx(dir.path(), false)).unwrap();
    let s = read_json(&dir.path().join(SETTINGS));
    assert_eq!(s["hooks"]["AfterAgent"], json!([]));
}

#[test]
fn second_install_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    install(&ctx(dir.path(), true)).unwrap();
    let before = std::fs::read(dir.path().join(SETTINGS)).unwrap();
    let agents = std::fs::read(dir.path().join("AGENTS.md")).unwrap();
    let lines = install(&ctx(dir.path(), true)).unwrap();
    assert!(
        lines.iter().all(|l| l.contains("already set up")),
        "{lines:?}"
    );
    assert_eq!(std::fs::read(dir.path().join(SETTINGS)).unwrap(), before);
    assert_eq!(std::fs::read(dir.path().join("AGENTS.md")).unwrap(), agents);
}

#[test]
fn install_keeps_user_settings_hooks_and_context() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(SETTINGS);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        r#"{"theme":"Dracula","context":{"fileName":"CONTEXT.md"},
           "mcpServers":{"other":{"command":"x"}},
           "hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"mine.sh"}]}]}}"#,
    )
    .unwrap();
    std::fs::write(dir.path().join("AGENTS.md"), "# Mine\n").unwrap();
    let lines = install(&ctx(dir.path(), false)).unwrap();
    assert!(lines[0].contains("Updated: .gemini/settings.json"));
    let s = read_json(&path);
    assert_eq!(s["theme"], "Dracula");
    assert_eq!(s["mcpServers"]["other"]["command"], "x");
    assert_eq!(s["context"]["fileName"], json!(["AGENTS.md", "CONTEXT.md"]));
    let start = commands(&s, "SessionStart");
    assert_eq!(start.len(), 2);
    assert_eq!(start[0], "mine.sh");
    let agents = std::fs::read_to_string(dir.path().join("AGENTS.md")).unwrap();
    assert!(agents.starts_with("# Mine\n"));
    assert!(agents.contains("post-cortex:begin"));
}

#[test]
fn jsonc_settings_are_skipped_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(SETTINGS);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "{ // mine\n}").unwrap();
    let lines = install(&ctx(dir.path(), false)).unwrap();
    assert!(lines[0].starts_with("  Skipped: "), "{lines:?}");
    assert!(lines[1].contains("Created: AGENTS.md"), "{lines:?}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ // mine\n}");
}

#[test]
fn mcp_config_names_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let text = mcp_config(&ctx(dir.path(), false));
    assert!(text.starts_with("# Merge into .gemini/settings.json"));
    let json_start = text.find("\n{").unwrap() + 1;
    let v: Value = serde_json::from_str(&text[json_start..]).unwrap();
    assert_eq!(
        v["mcpServers"]["post-cortex"]["httpUrl"],
        "http://127.0.0.1:3737/mcp"
    );
}

#[test]
fn read_path_is_relative_to_cwd() {
    let input = json!({"cwd": "/p", "tool_name": "read_file",
                       "tool_input": {"file_path": "/p/src/a.rs"}});
    assert_eq!(read_path(&input).as_deref(), Some("src/a.rs"));
    let input = json!({"tool_input": {"file_path": "src/b.rs"}});
    assert_eq!(read_path(&input).as_deref(), Some("src/b.rs"));
    assert_eq!(read_path(&json!({"tool_input": {}})), None);
}

#[test]
fn captured_turns_hold_prompt_and_reply() {
    let input = json!({"session_id": "g1", "prompt": "hi", "prompt_response": "hello"});
    let turns = captured_turns(&input);
    assert_eq!(turns.len(), 2);
    assert_eq!(turns[0].role, "user");
    assert_eq!(turns[0].turn_id, turn_id("g1", "user", "hi"));
    assert_eq!(turns[1].role, "assistant");
    assert_eq!(turns[1].text, "hello");
    assert_eq!(turns[1].source_session.as_deref(), Some("g1"));
    assert!(captured_turns(&json!({"prompt": " "})).is_empty());
}

#[tokio::test]
async fn hooks_fail_open_when_the_daemon_is_down() {
    let env = offline();
    for event in ["session-start", "file-context", "capture", "unknown"] {
        let input = json!({"session_id": "g1", "prompt": "hi", "prompt_response": "ok",
                           "tool_input": {"file_path": "a.rs"}});
        assert_eq!(hook(event, input, &env).await.unwrap(), None, "{event}");
    }
}

#[tokio::test]
async fn prompt_hook_returns_the_rules_reminder() {
    let out = hook("prompt", json!({"prompt": "hi"}), &offline())
        .await
        .unwrap()
        .unwrap();
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["hookSpecificOutput"]["hookEventName"], "BeforeAgent");
    assert!(
        v["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .starts_with("[PCX] Session: S1")
    );
}

/// A global setup: user-level settings and AGENTS.md, no ids.
#[test]
fn global_install_writes_user_files_without_ids() {
    let home = tempfile::tempdir().unwrap();
    let global = SetupContext {
        global: true,
        ..remote(ctx(home.path(), false))
    };
    let lines = install(&global).unwrap();
    assert!(lines[0].contains("~/.gemini/settings.json"), "{lines:?}");
    let settings = std::fs::read_to_string(home.path().join(".gemini/settings.json")).unwrap();
    assert!(
        settings.contains("pcx hook gemini session-start --profile cloud"),
        "{settings}"
    );
    assert!(!settings.contains("--session"), "{settings}");
    let agents = std::fs::read_to_string(home.path().join(".gemini/AGENTS.md")).unwrap();
    assert!(!agents.contains("W1"), "{agents}");
    assert!(!home.path().join("AGENTS.md").exists());
}
