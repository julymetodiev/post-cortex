// Copyright (c) 2026 Julius ML
// MIT License

use super::*;
use crate::clients::common::remote;
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
        "codex",
    )
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn commands(hooks: &Value, event: &str) -> Vec<String> {
    hooks["hooks"][event]
        .as_array()
        .map(|groups| {
            groups
                .iter()
                .flat_map(|g| g["hooks"].as_array().cloned().unwrap_or_default())
                .filter_map(|h| h["command"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn remote_install_writes_command_and_args_and_no_token() {
    let dir = tempfile::tempdir().unwrap();
    install(&remote(ctx(dir.path(), false))).unwrap();
    let text = std::fs::read_to_string(dir.path().join(".codex/config.toml")).unwrap();
    let config: toml::Value = toml::from_str(&text).unwrap();
    let server = &config["mcp_servers"]["post-cortex"];
    assert_eq!(server["command"].as_str(), Some("pcx"));
    let args: Vec<&str> = server["args"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|a| a.as_str())
        .collect();
    assert_eq!(args, ["mcp", "--profile", "cloud", "--author", "codex"]);
    assert!(server.get("url").is_none());
    assert!(!text.contains("PC_AUTH_TOKEN"), "{text}");
    let hooks = std::fs::read_to_string(dir.path().join(".codex/hooks.json")).unwrap();
    assert!(
        hooks.contains("--profile cloud") && !hooks.contains("--url"),
        "{hooks}"
    );

    // A section from a self-hosted setup is replaced: a re-run switches
    // the backend.
    let other = tempfile::tempdir().unwrap();
    install(&ctx(other.path(), false)).unwrap();
    let lines = install(&remote(ctx(other.path(), false))).unwrap();
    assert!(lines[0].contains("Updated"), "{lines:?}");
    let text = std::fs::read_to_string(other.path().join(".codex/config.toml")).unwrap();
    let config: toml::Value = toml::from_str(&text).unwrap();
    assert_eq!(
        config["mcp_servers"]["post-cortex"]["command"].as_str(),
        Some("pcx")
    );
    assert!(!text.contains("bearer_token_env_var"), "{text}");
    let hooks = std::fs::read_to_string(other.path().join(".codex/hooks.json")).unwrap();
    assert!(!hooks.contains("--url"), "{hooks}");
}

/// A global setup writes the user-level files, without ids: the hooks
/// resolve the project.
#[test]
fn global_install_has_no_ids() {
    let home = tempfile::tempdir().unwrap();
    let global = SetupContext {
        global: true,
        ..remote(ctx(home.path(), false))
    };
    let lines = install(&global).unwrap();
    assert!(lines[0].contains("~/.codex/config.toml"), "{lines:?}");
    let agents = std::fs::read_to_string(home.path().join(".codex/AGENTS.md")).unwrap();
    assert!(!agents.contains("S1") && !agents.contains("W1"), "{agents}");
    assert!(agents.contains("given at session start"), "{agents}");
    let hooks = std::fs::read_to_string(home.path().join(".codex/hooks.json")).unwrap();
    assert!(
        hooks.contains("pcx hook codex session-start --profile cloud"),
        "{hooks}"
    );
    assert!(!hooks.contains("--session"), "{hooks}");
    assert!(
        home.path()
            .join(".agents/skills/post-cortex-memory/SKILL.md")
            .exists()
    );
}

#[test]
fn install_writes_config_agents_hooks_and_skill() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx(dir.path(), false);
    let lines = install(&ctx).unwrap();
    assert_eq!(lines.len(), 4);
    assert!(lines.iter().all(|l| l.contains("Created")), "{lines:?}");

    let config: toml::Value =
        toml::from_str(&std::fs::read_to_string(dir.path().join(".codex/config.toml")).unwrap())
            .unwrap();
    let server = &config["mcp_servers"]["post-cortex"];
    assert_eq!(server["url"].as_str(), Some("http://127.0.0.1:3737/mcp"));
    assert_eq!(
        server["bearer_token_env_var"].as_str(),
        Some("PC_AUTH_TOKEN")
    );
    assert_eq!(
        server["http_headers"]["X-PCX-Author"].as_str(),
        Some("codex")
    );

    let agents = std::fs::read_to_string(dir.path().join("AGENTS.md")).unwrap();
    assert!(agents.contains("S1") && agents.contains("W1"));

    let hooks = read_json(&dir.path().join(".codex/hooks.json"));
    assert_eq!(
        hooks["hooks"]["SessionStart"][0]["matcher"],
        "startup|resume|compact"
    );
    assert_eq!(hooks["hooks"]["SessionStart"][0]["hooks"][0]["timeout"], 30);
    assert_eq!(
        commands(&hooks, "SessionStart"),
        vec![ctx.hook_command(Client::Codex, "session-start")]
    );
    assert_eq!(
        hooks["hooks"]["PreToolUse"][0]["matcher"],
        "apply_patch|Edit|Write"
    );
    assert_eq!(
        commands(&hooks, "UserPromptSubmit"),
        vec![ctx.hook_command(Client::Codex, "prompt")]
    );
    assert!(hooks["hooks"].get("Stop").is_none());

    assert!(
        dir.path()
            .join(".agents/skills/post-cortex-memory/SKILL.md")
            .exists()
    );
}

#[test]
fn second_install_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx(dir.path(), true);
    install(&ctx).unwrap();
    let files = [".codex/config.toml", "AGENTS.md", ".codex/hooks.json"];
    let before: Vec<Vec<u8>> = files
        .iter()
        .map(|f| std::fs::read(dir.path().join(f)).unwrap())
        .collect();
    let lines = install(&ctx).unwrap();
    assert!(
        lines.iter().all(|l| l.contains("already set up")),
        "{lines:?}"
    );
    let after: Vec<Vec<u8>> = files
        .iter()
        .map(|f| std::fs::read(dir.path().join(f)).unwrap())
        .collect();
    assert_eq!(before, after);
}

#[test]
fn install_keeps_user_config_and_hooks() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".codex")).unwrap();
    std::fs::write(
        dir.path().join(".codex/config.toml"),
        "# mine\nmodel = \"o3\"\n",
    )
    .unwrap();
    std::fs::write(dir.path().join("AGENTS.md"), "# Rules\n\nBe brief.\n").unwrap();
    std::fs::write(
        dir.path().join(".codex/hooks.json"),
        r#"{"hooks":{"SessionStart":[{"matcher":"startup","hooks":[{"type":"command","command":"echo hi"}]}]},"other":1}"#,
    )
    .unwrap();

    let ctx = ctx(dir.path(), true);
    install(&ctx).unwrap();

    let config = std::fs::read_to_string(dir.path().join(".codex/config.toml")).unwrap();
    assert!(config.starts_with("# mine\nmodel = \"o3\"\n"));
    assert!(config.contains(MCP_HEADER));
    let agents = std::fs::read_to_string(dir.path().join("AGENTS.md")).unwrap();
    assert!(agents.starts_with("# Rules\n\nBe brief.\n"));

    let hooks = read_json(&dir.path().join(".codex/hooks.json"));
    assert_eq!(hooks["other"], 1);
    assert_eq!(
        commands(&hooks, "SessionStart"),
        vec![
            "echo hi".to_string(),
            ctx.hook_command(Client::Codex, "session-start")
        ]
    );
    assert_eq!(
        commands(&hooks, "UserPromptSubmit"),
        vec![
            ctx.hook_command(Client::Codex, "prompt"),
            ctx.hook_command(Client::Codex, "capture-prompt")
        ]
    );
    assert_eq!(
        commands(&hooks, "Stop"),
        vec![ctx.hook_command(Client::Codex, "stop")]
    );

    // Without capture, the capture hooks go; the person's hook stays.
    let plain = self::ctx(dir.path(), false);
    install(&plain).unwrap();
    let hooks = read_json(&dir.path().join(".codex/hooks.json"));
    assert_eq!(
        commands(&hooks, "UserPromptSubmit"),
        vec![plain.hook_command(Client::Codex, "prompt")]
    );
    assert!(commands(&hooks, "Stop").is_empty());
    assert_eq!(commands(&hooks, "SessionStart")[0], "echo hi");
}

#[test]
fn mcp_config_names_the_file_and_the_section() {
    let dir = tempfile::tempdir().unwrap();
    let text = mcp_config(&ctx(dir.path(), false));
    assert!(text.starts_with("# Add to .codex/config.toml"));
    let toml_part: String = text
        .lines()
        .filter(|l| !l.starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");
    let parsed: toml::Value = toml::from_str(&toml_part).unwrap();
    assert_eq!(
        parsed["mcp_servers"]["post-cortex"]["url"].as_str(),
        Some("http://127.0.0.1:3737/mcp")
    );
}

#[test]
fn edited_paths_are_relative_to_cwd() {
    let input = json!({
        "cwd": "/repo",
        "tool_name": "apply_patch",
        "tool_input": { "command": "*** Begin Patch\n*** Update File: /repo/src/a.rs\n*** Add File: lib/b.rs\n*** End Patch" }
    });
    assert_eq!(edited_paths(&input), vec!["src/a.rs", "lib/b.rs"]);
    let write = json!({ "cwd": "/repo", "tool_input": { "file_path": "/repo/x.rs" } });
    assert_eq!(edited_paths(&write), vec!["x.rs"]);
    assert!(edited_paths(&json!({})).is_empty());
}

#[test]
fn captured_turns_use_the_codex_turn_id() {
    let prompt = json!({ "session_id": "c1", "turn_id": "t7", "prompt": " fix it " });
    let turn = captured_turn(&prompt, "user").unwrap();
    assert_eq!(turn.turn_id, "t7:user");
    assert_eq!(turn.text, "fix it");
    assert_eq!(turn.source_session.as_deref(), Some("c1"));

    let stop = json!({ "session_id": "c1", "last_assistant_message": "done" });
    let turn = captured_turn(&stop, "assistant").unwrap();
    assert_eq!(turn.turn_id, turn_id("c1", "assistant", "done"));
    assert!(captured_turn(&json!({ "last_assistant_message": "" }), "assistant").is_none());
    assert!(captured_turn(&json!({}), "user").is_none());
}

#[tokio::test]
async fn hooks_fail_open_when_the_daemon_is_down() {
    let env = offline();
    assert_eq!(hook("session-start", json!({}), &env).await, Ok(None));
    let edit = json!({ "cwd": "/repo", "tool_input": { "command": "*** Update File: a.rs" } });
    assert_eq!(hook("pre-edit", edit, &env).await, Ok(None));
    let prompt = json!({ "session_id": "c", "turn_id": "t", "prompt": "hi" });
    assert_eq!(hook("capture-prompt", prompt, &env).await, Ok(None));
    let stop = json!({ "session_id": "c", "last_assistant_message": "ok" });
    assert_eq!(hook("stop", stop, &env).await, Ok(None));
    assert_eq!(hook("unknown", json!({}), &env).await, Ok(None));
}

#[tokio::test]
async fn prompt_hook_returns_the_rules_reminder() {
    let env = offline();
    let out = hook("prompt", json!({ "prompt": "hi" }), &env)
        .await
        .unwrap()
        .unwrap();
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["hookSpecificOutput"]["hookEventName"], "UserPromptSubmit");
    assert_eq!(
        v["hookSpecificOutput"]["additionalContext"],
        env.rules_reminder().await
    );
}
