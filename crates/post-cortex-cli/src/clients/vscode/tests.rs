// Copyright (c) 2026 Julius ML
// MIT License

use std::path::Path;

use super::*;
use crate::clients::common::{cloud_args, remote};

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
        "copilot",
    )
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn remote_install_writes_the_stdio_entry_and_no_token() {
    let dir = tempfile::tempdir().unwrap();
    install(&ctx(dir.path(), false)).unwrap();
    install(&remote(ctx(dir.path(), false))).unwrap();
    let text = std::fs::read_to_string(dir.path().join(MCP)).unwrap();
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        v["servers"]["post-cortex"],
        json!({ "type": "stdio", "command": "pcx", "args": cloud_args(Client::Vscode) })
    );
    assert_eq!(v["inputs"], json!([]));
    assert!(!text.contains("Bearer"), "{text}");
    let hooks = std::fs::read_to_string(dir.path().join(HOOKS)).unwrap();
    assert!(hooks.contains("--profile cloud") && !hooks.contains("--url"));
}

#[test]
fn install_writes_mcp_agents_hooks_and_skill() {
    let dir = tempfile::tempdir().unwrap();
    let lines = install(&ctx(dir.path(), false)).unwrap();
    assert_eq!(lines.len(), 4);
    assert!(lines[0].contains("Created: .vscode/mcp.json"));

    let mcp = read_json(&dir.path().join(MCP));
    let server = &mcp["servers"]["post-cortex"];
    assert_eq!(server["type"], "http");
    assert_eq!(server["url"], "http://127.0.0.1:3737/mcp");
    assert_eq!(server["headers"]["X-PCX-Author"], "copilot");
    assert_eq!(
        server["headers"]["Authorization"],
        "Bearer ${input:pc-auth-token}"
    );
    assert_eq!(mcp["inputs"][0]["id"], INPUT_ID);
    assert_eq!(mcp["inputs"][0]["password"], true);

    let hooks = read_json(&dir.path().join(HOOKS));
    assert_eq!(
        hooks["hooks"]["SessionStart"][0]["command"],
        "pcx hook vscode session-start --session S1 --workspace W1 --profile self --url http://127.0.0.1:3737"
    );
    assert_eq!(hooks["hooks"]["PreToolUse"][0]["type"], "command");
    assert_eq!(hooks["hooks"]["PreToolUse"][0]["timeout"], 10);
    assert!(hooks["hooks"].get("Stop").is_none());
    assert!(hooks["hooks"].get("UserPromptSubmit").is_none());

    let agents = std::fs::read_to_string(dir.path().join("AGENTS.md")).unwrap();
    assert!(agents.contains("`S1`"));
    assert!(
        dir.path()
            .join(".agents/skills/post-cortex-memory/SKILL.md")
            .exists()
    );
}

#[test]
fn capture_adds_prompt_and_stop_hooks() {
    let dir = tempfile::tempdir().unwrap();
    install(&ctx(dir.path(), true)).unwrap();
    let hooks = read_json(&dir.path().join(HOOKS));
    assert!(
        hooks["hooks"]["Stop"][0]["command"]
            .as_str()
            .unwrap()
            .starts_with("pcx hook vscode stop ")
    );
    assert!(
        hooks["hooks"]["UserPromptSubmit"][0]["command"]
            .as_str()
            .unwrap()
            .starts_with("pcx hook vscode capture-prompt ")
    );
}

#[test]
fn second_install_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    install(&ctx(dir.path(), true)).unwrap();
    let files = [MCP, HOOKS, "AGENTS.md"].map(|f| std::fs::read(dir.path().join(f)).unwrap());
    let lines = install(&ctx(dir.path(), true)).unwrap();
    assert!(
        lines.iter().all(|l| l.contains("already set up")),
        "{lines:?}"
    );
    for (f, before) in [MCP, HOOKS, "AGENTS.md"].iter().zip(files) {
        assert_eq!(std::fs::read(dir.path().join(f)).unwrap(), before, "{f}");
    }
}

#[test]
fn install_keeps_user_servers_and_inputs() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(MCP);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        r#"{"servers":{"gh":{"type":"http","url":"u"}},
           "inputs":[{"type":"promptString","id":"gh-token"}]}"#,
    )
    .unwrap();
    let lines = install(&ctx(dir.path(), false)).unwrap();
    assert!(lines[0].contains("Updated: .vscode/mcp.json"));
    let mcp = read_json(&path);
    assert_eq!(mcp["servers"]["gh"]["url"], "u");
    let ids: Vec<&str> = mcp["inputs"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|i| i["id"].as_str())
        .collect();
    assert_eq!(ids, vec!["gh-token", INPUT_ID]);
}

#[test]
fn jsonc_mcp_json_is_skipped_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(MCP);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "{ // mine\n}").unwrap();
    let lines = install(&ctx(dir.path(), false)).unwrap();
    assert!(lines[0].starts_with("  Skipped: "), "{lines:?}");
    assert!(dir.path().join(HOOKS).exists());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ // mine\n}");
}

#[test]
fn mcp_config_names_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let text = mcp_config(&ctx(dir.path(), false));
    assert!(text.starts_with("# Merge into .vscode/mcp.json"));
    let v: Value = serde_json::from_str(&text[text.find('{').unwrap()..]).unwrap();
    assert_eq!(
        v["servers"]["post-cortex"]["url"],
        "http://127.0.0.1:3737/mcp"
    );
    assert_eq!(v["inputs"][0]["id"], INPUT_ID);
}

#[test]
fn edited_paths_cover_the_edit_tools() {
    let input = json!({"cwd": "/p", "tool_name": "replace_string_in_file",
                       "tool_input": {"filePath": "/p/src/a.rs", "oldString": "x"}});
    assert_eq!(edited_paths(&input), vec!["src/a.rs"]);
    let input = json!({"tool_name": "multi_replace_string_in_file",
                       "tool_input": {"replacements": [{"filePath": "a.rs"}, {"filePath": "b.rs"}, {"filePath": "a.rs"}]}});
    assert_eq!(edited_paths(&input), vec!["a.rs", "b.rs"]);
    let input = json!({"tool_name": "apply_patch",
                       "tool_input": {"input": "*** Begin Patch\n*** Update File: c.rs\n@@\n*** End Patch"}});
    assert_eq!(edited_paths(&input), vec!["c.rs"]);
    let input = json!({"tool_name": "create_file", "tool_input": {"filePath": "d.rs"}});
    assert_eq!(edited_paths(&input), vec!["d.rs"]);
    let input = json!({"tool_name": "read_file", "tool_input": {"filePath": "a.rs"}});
    assert!(edited_paths(&input).is_empty());
}

#[test]
fn last_reply_reads_jsonl_and_json_transcripts() {
    let jsonl = "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"q\"}}\n\
                 {\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"first\"}]}}\n\
                 not json\n\
                 {\"role\":\"assistant\",\"content\":\"last\"}\n";
    assert_eq!(last_reply(jsonl).as_deref(), Some("last"));
    let chat =
        r#"{"requests":[{"message":{"text":"q"},"response":[{"value":"an"},{"value":"swer"}]}]}"#;
    assert_eq!(last_reply(chat).as_deref(), Some("answer"));
    assert_eq!(last_reply("{\"x\":1}"), None);
}

#[test]
fn captured_turns_from_prompt_and_transcript() {
    let turn = captured_turn(&json!({"session_id": "v1", "prompt": " hi "}), "user").unwrap();
    assert_eq!(turn.text, "hi");
    assert_eq!(turn.turn_id, turn_id("v1", "user", "hi"));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.jsonl");
    std::fs::write(&path, "{\"role\":\"assistant\",\"content\":\"done\"}\n").unwrap();
    let input = json!({"transcript_path": path.to_str().unwrap()});
    let turn = captured_turn(&input, "assistant").unwrap();
    assert_eq!(turn.role, "assistant");
    assert_eq!(turn.text, "done");
    assert!(captured_turn(&json!({"transcript_path": "/nope"}), "assistant").is_none());
}

#[tokio::test]
async fn hooks_fail_open_when_the_daemon_is_down() {
    let env = offline();
    let input = json!({"prompt": "hi", "tool_name": "create_file",
                       "tool_input": {"filePath": "a.rs"}});
    for event in [
        "session-start",
        "pre-edit",
        "capture-prompt",
        "stop",
        "unknown",
    ] {
        assert_eq!(
            hook(event, input.clone(), &env).await.unwrap(),
            None,
            "{event}"
        );
    }
}
