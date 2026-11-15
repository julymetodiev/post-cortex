// Copyright (c) 2026 Julius ML
// MIT License

use super::*;
use crate::clients::common::{cloud_args, is_our_hook, remote};
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

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn offline() -> HookEnv {
    HookEnv::new(
        "http://127.0.0.1:9",
        reqwest::Client::new(),
        "S1",
        "W1",
        "cursor",
    )
}

#[test]
fn install_writes_mcp_rule_hooks_and_skill() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx(dir.path(), false);
    let lines = install(&ctx).unwrap();
    assert_eq!(lines.len(), 4);
    assert!(
        lines.iter().all(|l| l.starts_with("  Created: ")),
        "{lines:?}"
    );

    let mcp = read_json(&dir.path().join(MCP_JSON));
    let server = &mcp["mcpServers"]["post-cortex"];
    assert_eq!(server["url"], "http://127.0.0.1:3737/mcp");
    assert_eq!(server["headers"]["X-PCX-Author"], "cursor");
    assert_eq!(
        server["headers"]["Authorization"],
        "Bearer ${env:PC_AUTH_TOKEN}"
    );

    let rule = std::fs::read_to_string(dir.path().join(RULE_FILE)).unwrap();
    assert!(
        rule.starts_with("---\ndescription: post-cortex memory rules\nalwaysApply: true\n---\n\n")
    );
    assert!(rule.contains("`S1`") && rule.contains("`W1`"));
    assert!(!dir.path().join("AGENTS.md").exists());

    let hooks = read_json(&dir.path().join(HOOKS_JSON));
    assert_eq!(hooks["version"], 1);
    assert_eq!(
        hooks["hooks"]["sessionStart"][0]["command"],
        "pcx hook cursor sessionStart --session S1 --workspace W1 --profile self --url http://127.0.0.1:3737"
    );
    assert_eq!(hooks["hooks"]["postToolUse"][0]["matcher"], "Read");
    assert!(hooks["hooks"].get("beforeSubmitPrompt").is_none());
    assert!(hooks["hooks"].get("afterAgentResponse").is_none());

    assert!(
        dir.path()
            .join(".agents/skills/post-cortex-memory/SKILL.md")
            .exists()
    );
}

#[test]
fn remote_install_writes_the_stdio_entry_and_no_token() {
    let dir = tempfile::tempdir().unwrap();
    let mcp = dir.path().join(MCP_JSON);
    // An HTTP entry from a self-hosted setup becomes the stdio one.
    install(&ctx(dir.path(), false)).unwrap();
    install(&remote(ctx(dir.path(), false))).unwrap();
    let server = &read_json(&mcp)["mcpServers"]["post-cortex"];
    assert_eq!(
        *server,
        json!({ "command": "pcx", "args": cloud_args(Client::Cursor) })
    );
    let hooks = read_json(&dir.path().join(HOOKS_JSON));
    assert_eq!(
        hooks["hooks"]["sessionStart"][0]["command"],
        "pcx hook cursor sessionStart --session S1 --workspace W1 --profile cloud"
    );
    for file in [MCP_JSON, HOOKS_JSON] {
        let text = std::fs::read_to_string(dir.path().join(file)).unwrap();
        assert!(
            !text.contains("Bearer") && !text.contains("PC_AUTH_TOKEN"),
            "{text}"
        );
    }
}

#[test]
fn second_install_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx(dir.path(), true);
    install(&ctx).unwrap();
    let files = [MCP_JSON, RULE_FILE, HOOKS_JSON];
    let before: Vec<Vec<u8>> = files
        .iter()
        .map(|f| std::fs::read(dir.path().join(f)).unwrap())
        .collect();
    let lines = install(&ctx).unwrap();
    assert!(
        lines
            .iter()
            .all(|l| l.ends_with("already set up, left as it is")),
        "{lines:?}"
    );
    for (f, bytes) in files.iter().zip(before) {
        assert_eq!(std::fs::read(dir.path().join(f)).unwrap(), bytes, "{f}");
    }
}

#[test]
fn install_keeps_user_config_and_hooks() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".cursor")).unwrap();
    std::fs::write(
        dir.path().join(MCP_JSON),
        r#"{"mcpServers":{"other":{"command":"x"}}}"#,
    )
    .unwrap();
    std::fs::write(
        dir.path().join(HOOKS_JSON),
        r#"{"version":1,"hooks":{"sessionStart":[{"command":"./mine.sh"},{"command":"pcx hook cursor sessionStart --session OLD"}],"stop":[{"command":"./stop.sh"}]}}"#,
    )
    .unwrap();

    // With capture, then without: our capture hooks go, the user's stay.
    install(&ctx(dir.path(), true)).unwrap();
    let hooks = read_json(&dir.path().join(HOOKS_JSON));
    assert_eq!(
        hooks["hooks"]["beforeSubmitPrompt"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        hooks["hooks"]["afterAgentResponse"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    install(&ctx(dir.path(), false)).unwrap();

    let mcp = read_json(&dir.path().join(MCP_JSON));
    assert_eq!(mcp["mcpServers"]["other"]["command"], "x");
    assert!(mcp["mcpServers"]["post-cortex"].is_object());

    let hooks = read_json(&dir.path().join(HOOKS_JSON));
    let start = hooks["hooks"]["sessionStart"].as_array().unwrap();
    assert_eq!(start.len(), 2);
    assert_eq!(start[0]["command"], "./mine.sh");
    // An entry an older setup wrote (plain `pcx`) is replaced, not kept.
    assert!(is_our_hook(&start[1], Client::Cursor));
    assert!(
        start[1]["command"]
            .as_str()
            .unwrap()
            .contains("--session S1")
    );
    assert_eq!(hooks["hooks"]["stop"][0]["command"], "./stop.sh");
    assert_eq!(hooks["hooks"]["beforeSubmitPrompt"], json!([]));
    assert_eq!(hooks["hooks"]["afterAgentResponse"], json!([]));
}

#[test]
fn install_skips_skill_present_elsewhere_and_refuses_jsonc() {
    let dir = tempfile::tempdir().unwrap();
    let skill = dir.path().join(".claude/skills/post-cortex-memory");
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(skill.join("SKILL.md"), "x").unwrap();
    let lines = install(&ctx(dir.path(), false)).unwrap();
    assert!(lines[3].contains(".claude/skills/post-cortex-memory already set up"));
    assert!(!dir.path().join(".agents/skills").exists());

    std::fs::write(dir.path().join(MCP_JSON), "{ // mine\n}").unwrap();
    let lines = install(&ctx(dir.path(), false)).unwrap();
    assert!(lines[0].starts_with("  Skipped: "), "{lines:?}");
    assert!(lines[1].contains(RULE_FILE), "{lines:?}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join(MCP_JSON)).unwrap(),
        "{ // mine\n}"
    );
}

#[test]
fn mcp_config_has_snippet_and_install_link() {
    let dir = tempfile::tempdir().unwrap();
    let text = mcp_config(&ctx(dir.path(), false));
    assert!(text.starts_with("# Add to .cursor/mcp.json"));
    let json_start = text.find('{').unwrap();
    let json_end = text.find("\n\n").unwrap();
    let file: Value = serde_json::from_str(&text[json_start..json_end]).unwrap();
    assert_eq!(
        file["mcpServers"]["post-cortex"]["url"],
        "http://127.0.0.1:3737/mcp"
    );
    assert!(
        text.contains("cursor://anysphere.cursor-deeplink/mcp/install?name=post-cortex&config=eyJ")
    );
}

#[test]
fn base64_matches_rfc4648() {
    assert_eq!(base64(b""), "");
    assert_eq!(base64(b"f"), "Zg==");
    assert_eq!(base64(b"fo"), "Zm8=");
    assert_eq!(base64(b"foo"), "Zm9v");
    assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    // The example from Cursor's install-link docs.
    assert_eq!(
        base64(
            br#"{"command":"npx","args":["-y","@modelcontextprotocol/server-postgres","postgresql://localhost/mydb"]}"#
        ),
        "eyJjb21tYW5kIjoibnB4IiwiYXJncyI6WyIteSIsIkBtb2RlbGNvbnRleHRwcm90b2NvbC9zZXJ2ZXItcG9zdGdyZXMiLCJwb3N0Z3Jlc3FsOi8vbG9jYWxob3N0L215ZGIiXX0="
    );
}

#[test]
fn read_path_is_relative_to_the_workspace() {
    let input = json!({
        "tool_name": "Read",
        "tool_input": { "file_path": "/repo/src/main.rs" },
        "workspace_roots": ["/repo"],
    });
    assert_eq!(read_path(&input).as_deref(), Some("src/main.rs"));
    let input = json!({
        "tool_name": "Read",
        "tool_input": { "path": "/elsewhere/a.rs" },
        "cwd": "/repo",
    });
    assert_eq!(read_path(&input).as_deref(), Some("/elsewhere/a.rs"));
    let input = json!({ "tool_name": "Read", "tool_input": { "target_file": "./b.rs" } });
    assert_eq!(read_path(&input).as_deref(), Some("b.rs"));
    let input = json!({ "tool_name": "Shell", "tool_input": { "command": "ls" } });
    assert_eq!(read_path(&input), None);
}

#[test]
fn turns_are_keyed_on_conversation_and_generation() {
    let input = json!({
        "conversation_id": "c1",
        "generation_id": "g1",
        "prompt": " fix it ",
        "text": "done",
    });
    let user = prompt_turn(&input).unwrap();
    assert_eq!(user.turn_id, "c1:g1:user");
    assert_eq!(user.role, "user");
    assert_eq!(user.text, "fix it");
    assert_eq!(user.source_session.as_deref(), Some("c1"));
    let reply = response_turn(&input).unwrap();
    assert_eq!(reply.turn_id, "c1:g1:assistant");
    assert_eq!(reply.role, "assistant");

    let bare = json!({ "text": "hi" });
    assert_eq!(
        response_turn(&bare).unwrap().turn_id,
        turn_id("cursor", "assistant", "hi")
    );
    assert!(prompt_turn(&json!({ "prompt": "  " })).is_none());
}

#[tokio::test]
async fn hooks_fail_open_when_the_daemon_is_down() {
    let env = offline();
    let start = json!({ "hook_event_name": "sessionStart", "session_id": "x" });
    assert_eq!(hook("sessionStart", start, &env).await.unwrap(), None);
    let read = json!({ "tool_name": "Read", "tool_input": { "file_path": "a.rs" } });
    assert_eq!(hook("postToolUse", read, &env).await.unwrap(), None);
    let prompt = json!({ "conversation_id": "c", "generation_id": "g", "prompt": "hi" });
    let out = hook("beforeSubmitPrompt", prompt, &env)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&out).unwrap(),
        json!({ "continue": true })
    );
    let reply = json!({ "conversation_id": "c", "generation_id": "g", "text": "ok" });
    assert_eq!(hook("afterAgentResponse", reply, &env).await.unwrap(), None);
    assert_eq!(hook("stop", json!({}), &env).await.unwrap(), None);
}

#[test]
fn additional_context_shape() {
    let out: Value = serde_json::from_str(&additional_context("x".into())).unwrap();
    assert_eq!(out, json!({ "additional_context": "x" }));
}

/// A global setup: user-level mcp.json and hooks.json, no rule file
/// (Cursor reads none at user level), the skill in ~/.cursor/skills.
#[test]
fn global_install_writes_user_files_without_ids() {
    let home = tempfile::tempdir().unwrap();
    let global = SetupContext {
        global: true,
        ..remote(ctx(home.path(), false))
    };
    let lines = install(&global).unwrap();
    assert_eq!(lines.len(), 3, "{lines:?}");
    let hooks = std::fs::read_to_string(home.path().join(HOOKS_JSON)).unwrap();
    assert!(
        hooks.contains("pcx hook cursor sessionStart --profile cloud"),
        "{hooks}"
    );
    assert!(!hooks.contains("--session"), "{hooks}");
    assert!(!home.path().join(RULE_FILE).exists());
    assert!(
        home.path()
            .join(".cursor/skills/post-cortex-memory/SKILL.md")
            .exists()
    );
}
