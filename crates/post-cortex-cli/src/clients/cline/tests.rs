// Copyright (c) 2026 Julius ML
// MIT License

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

#[test]
fn install_writes_agents_md_skill_and_hooks_once() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx(dir.path(), false);
    let lines = install(&ctx).unwrap();
    assert!(lines.iter().any(|l| l.contains("Created: AGENTS.md")));
    assert!(lines.last().unwrap().contains("pcx mcp-config cline"));
    assert!(
        dir.path()
            .join(".agents/skills/post-cortex-memory/SKILL.md")
            .exists()
    );
    let hooks = dir.path().join(".clinerules/hooks");
    for name in [
        "TaskStart",
        "TaskResume",
        "UserPromptSubmit",
        "PreToolUse",
        "PostToolUse",
    ] {
        let script = std::fs::read_to_string(hooks.join(name)).unwrap();
        assert!(script.starts_with("#!/bin/sh\n"));
        assert!(
            script.contains(&ctx.hook_command(Client::Cline, name)),
            "{script}"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(hooks.join(name))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o111, 0o111);
        }
    }

    let before = std::fs::read(hooks.join("PreToolUse")).unwrap();
    let again = install(&ctx).unwrap();
    let (files, mcp) = again.split_at(again.len() - 1);
    assert!(
        files.iter().all(|l| l.contains("already set up")),
        "{again:?}"
    );
    assert!(mcp[0].contains("pcx mcp-config cline"));
    assert_eq!(std::fs::read(hooks.join("PreToolUse")).unwrap(), before);
}

#[test]
fn remote_profile_prints_stdio_and_hooks_name_the_profile() {
    let dir = tempfile::tempdir().unwrap();
    let cloud = remote(ctx(dir.path(), false));
    install(&cloud).unwrap();
    let script = std::fs::read_to_string(dir.path().join(".clinerules/hooks/TaskStart")).unwrap();
    assert!(
        script.contains("--profile cloud") && !script.contains("--url"),
        "{script}"
    );
    let text = mcp_config(&cloud);
    let v: Value = serde_json::from_str(&text[text.find("\n{").unwrap()..]).unwrap();
    assert_eq!(
        v["mcpServers"]["post-cortex"],
        json!({ "type": "stdio", "command": "pcx", "args": cloud_args(Client::Cline), "disabled": false })
    );
    assert!(!text.contains("Bearer <token>"), "{text}");
}

#[test]
fn capture_adds_prompt_capture() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx(dir.path(), true);
    install(&ctx).unwrap();
    let hooks = dir.path().join(".clinerules/hooks");
    let prompt = std::fs::read_to_string(hooks.join("UserPromptSubmit")).unwrap();
    assert!(prompt.contains(&ctx.hook_command(Client::Cline, PROMPT_CAPTURE)));
}

#[test]
fn user_hook_scripts_and_agents_md_are_kept() {
    let dir = tempfile::tempdir().unwrap();
    let hooks = dir.path().join(".clinerules/hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    std::fs::write(hooks.join("PreToolUse"), "#!/bin/sh\necho mine\n").unwrap();
    std::fs::write(dir.path().join("AGENTS.md"), "# Mine\n").unwrap();
    let lines = install(&ctx(dir.path(), false)).unwrap();
    assert!(
        lines
            .iter()
            .any(|l| l.contains("Skipped: .clinerules/hooks/PreToolUse"))
    );
    assert_eq!(
        std::fs::read_to_string(hooks.join("PreToolUse")).unwrap(),
        "#!/bin/sh\necho mine\n"
    );
    assert!(
        std::fs::read_to_string(dir.path().join("AGENTS.md"))
            .unwrap()
            .starts_with("# Mine\n")
    );
}

#[test]
fn edited_file_is_relative_to_the_workspace() {
    let input = json!({
        "hookName": "PreToolUse",
        "workspaceRoots": ["/repo"],
        "preToolUse": { "toolName": "replace_in_file", "parameters": { "path": "/repo/src/a.rs" } }
    });
    assert_eq!(edited_files(&input), vec!["src/a.rs"]);
    let relative = json!({
        "preToolUse": { "toolName": "write_to_file", "parameters": { "path": "src/b.rs" } }
    });
    assert_eq!(edited_files(&relative), vec!["src/b.rs"]);
    let patch = json!({
        "workspaceRoots": ["/repo"],
        "preToolUse": { "toolName": "apply_patch", "parameters": {
            "input": "*** Begin Patch\n*** Update File: /repo/src/c.rs\n@@\n-a\n+b\n*** Add File: src/e.rs\n*** End Patch" } }
    });
    assert_eq!(edited_files(&patch), vec!["src/c.rs", "src/e.rs"]);
    let read = json!({
        "preToolUse": { "toolName": "read_file", "parameters": { "path": "src/a.rs" } }
    });
    assert!(edited_files(&read).is_empty());
}

#[test]
fn read_files_are_the_paths_a_read_tool_names() {
    let many = json!({
        "workspaceRoots": ["/repo"],
        "postToolUse": { "toolName": "read_files", "parameters": {
            "files": "[{\"path\":\"/repo/src/a.rs\",\"start_line\":1},{\"path\":\"src/b.rs\"}]" } }
    });
    assert_eq!(read_files(&many), vec!["src/a.rs", "src/b.rs"]);
    let listed = json!({
        "postToolUse": { "toolName": "read_files", "parameters": { "files": [{ "path": "c.rs" }] } }
    });
    assert_eq!(read_files(&listed), vec!["c.rs"]);
    let one = json!({
        "postToolUse": { "toolName": "read_file", "parameters": { "path": "d.rs" } }
    });
    assert_eq!(read_files(&one), vec!["d.rs"]);
    let edit = json!({
        "postToolUse": { "toolName": "write_to_file", "parameters": { "path": "e.rs" } }
    });
    assert!(read_files(&edit).is_empty());
}

#[test]
fn capture_turns_parse_payloads() {
    let prompt = json!({ "taskId": "T1", "userPromptSubmit": { "prompt": " add tests " } });
    let turn = prompt_turn(&prompt).unwrap();
    assert_eq!((turn.role, turn.text.as_str()), ("user", "add tests"));
    assert_eq!(turn.turn_id, turn_id("T1", "user", "add tests"));
    assert_eq!(turn.source_session.as_deref(), Some("T1"));

    let done = json!({ "taskId": "T1", "postToolUse": {
        "toolName": "attempt_completion", "parameters": { "result": "All green." } } });
    let turn = answer_turn(&done).unwrap();
    assert_eq!((turn.role, turn.text.as_str()), ("assistant", "All green."));
    assert_eq!(turn.tools, vec!["attempt_completion".to_string()]);

    let other =
        json!({ "postToolUse": { "toolName": "read_file", "parameters": { "result": "x" } } });
    assert!(answer_turn(&other).is_none());
    assert!(prompt_turn(&json!({})).is_none());
}

#[tokio::test]
async fn hook_replies_and_fails_open() {
    let env = HookEnv::new(
        "http://127.0.0.1:9",
        reqwest::Client::new(),
        "S1",
        "W1",
        "cline",
    );
    let prompt = json!({ "taskId": "T1", "userPromptSubmit": { "prompt": "hi" } });
    for event in ["UserPromptSubmit", PROMPT_CAPTURE] {
        let out = hook(event, prompt.clone(), &env).await.unwrap().unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["cancel"], false);
        assert!(
            v["contextModification"]
                .as_str()
                .unwrap()
                .contains("Session: S1")
        );
    }
    let edit =
        json!({ "preToolUse": { "toolName": "write_to_file", "parameters": { "path": "a.rs" } } });
    for (event, input) in [
        ("TaskStart", json!({})),
        ("TaskResume", json!({})),
        ("PreToolUse", edit),
        (
            "PostToolUse",
            json!({ "postToolUse": { "toolName": "attempt_completion", "parameters": { "result": "ok" } } }),
        ),
        ("TaskCancel", json!({})),
    ] {
        assert_eq!(hook(event, input, &env).await, Ok(None), "{event}");
    }
}

#[test]
fn mcp_config_is_streamable_http() {
    let dir = tempfile::tempdir().unwrap();
    let text = mcp_config(&ctx(dir.path(), false));
    assert!(text.starts_with("// Merge into cline_mcp_settings.json"));
    let json_part: String = text
        .lines()
        .filter(|l| !l.starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    let v: Value = serde_json::from_str(&json_part).unwrap();
    let entry = &v["mcpServers"]["post-cortex"];
    assert_eq!(entry["type"], "streamableHttp");
    assert_eq!(entry["url"], "http://127.0.0.1:3737/mcp");
    assert_eq!(entry["headers"]["X-PCX-Author"], "cline");
}
