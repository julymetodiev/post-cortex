// Copyright (c) 2026 Julius ML
// MIT License

//! Cline: instructions, skill and hook scripts. The MCP entry is printed,
//! not written.
//!
//! - MCP: Cline keeps it in a user-level `cline_mcp_settings.json` whose
//!   path depends on the editor and OS (the CLI uses
//!   `~/.cline/data/settings/`, the VS Code extension its globalStorage), so
//!   setup does not guess it: the report says to run `pcx mcp-config cline`.
//!   The docs do not document env interpolation in headers, so the entry
//!   has no bearer token.
//! - Instructions: the root `AGENTS.md` (Cline reads it); the skill goes to
//!   `.agents/skills/`.
//! - Hooks: executable scripts named after the hook in `.clinerules/hooks/`
//!   (read by the VS Code extension and, as the older layout, by the CLI).
//!   Each runs `pcx hook cline <event>` with the payload on stdin and prints
//!   its JSON reply (`{"cancel":false}` when pcx has nothing to say).
//!   `TaskStart` / `TaskResume` inject the cards, `UserPromptSubmit` the
//!   rules reminder, `PreToolUse` the file memory (up to three files) before
//!   `write_to_file`,
//!   `replace_in_file`, `editor` and `apply_patch` (Cline shows it to the
//!   model on the next request). With `--capture`, `UserPromptSubmit` also
//!   stores the prompt and `PostToolUse` the answer of `attempt_completion` /
//!   `submit_and_exit`. Cline runs hooks only on macOS and Linux, once
//!   "Enable Hooks" is on in its settings. A script with the same name that
//!   post-cortex did not write is left alone.

use std::path::Path;

use serde_json::{Value, json};

use crate::clients::Client;
use crate::clients::common::{
    Change, HookEnv, McpEntry, REMOTE_TOKEN_NOTE, SetupContext, Turn, install_skill, patch_paths,
    project_relative, report, report_path, skipped, turn_id, upsert_block, write_owned,
};

/// The line marking a hook script as post-cortex's own.
const OWNED_MARK: &str = "# post-cortex hook, written by `pcx setup --client cline`";

/// `UserPromptSubmit` when setup ran with `--capture`.
const PROMPT_CAPTURE: &str = "UserPromptSubmit-capture";

/// Tools that change a file, whose path gets the file memory first.
const EDIT_TOOLS: [&str; 4] = ["write_to_file", "replace_in_file", "editor", "apply_patch"];

/// Files a single edit gets memory for, at most.
const MAX_FILES: usize = 3;

/// Tools whose input is the final answer of the task.
const ANSWER_TOOLS: [&str; 2] = ["attempt_completion", "submit_and_exit"];

pub fn install(ctx: &SetupContext) -> Result<Vec<String>, String> {
    let mut lines = Vec::new();
    let agents = ctx.project("AGENTS.md");
    lines.push(report(
        upsert_block(&agents, &ctx.instructions())?,
        &agents,
        ctx,
    ));
    lines.extend(install_skill(&ctx.project(".agents/skills"), ctx)?);

    let prompt_event = if ctx.capture {
        PROMPT_CAPTURE
    } else {
        "UserPromptSubmit"
    };
    let mut hooks = vec![
        ("TaskStart", "TaskStart"),
        ("TaskResume", "TaskResume"),
        ("UserPromptSubmit", prompt_event),
        ("PreToolUse", "PreToolUse"),
    ];
    if ctx.capture {
        hooks.push(("PostToolUse", "PostToolUse"));
    }
    for (name, event) in hooks {
        let path = ctx.project(&format!(".clinerules/hooks/{name}"));
        lines.push(write_hook(&path, &hook_script(ctx, event), ctx)?);
    }
    lines.push("  MCP: run `pcx mcp-config cline` and paste it into Cline's MCP settings (user-level, per editor)".into());
    Ok(lines)
}

pub fn mcp_config(ctx: &SetupContext) -> String {
    if let McpEntry::Stdio { command, args } = ctx.mcp_entry(Client::Cline) {
        let snippet = json!({ "mcpServers": { "post-cortex": {
            "type": "stdio",
            "command": command,
            "args": args,
            "disabled": false,
        }}});
        return format!(
            "// Merge into cline_mcp_settings.json (Cline > MCP Servers > Configure; CLI: ~/.cline/data/settings/).\n\
             // {REMOTE_TOKEN_NOTE}\n{}\n",
            serde_json::to_string_pretty(&snippet).unwrap_or_default()
        );
    }
    let snippet = json!({ "mcpServers": { "post-cortex": {
        "type": "streamableHttp",
        "url": ctx.mcp_url(),
        "headers": { "X-PCX-Author": Client::Cline.author() },
        "disabled": false,
    }}});
    format!(
        "// Merge into cline_mcp_settings.json (Cline > MCP Servers > Configure; CLI: ~/.cline/data/settings/).\n\
         // Cline does not expand env vars in headers: if the daemon has an auth_token, add\n\
         // \"Authorization\": \"Bearer <token>\" to headers yourself.\n{}\n",
        serde_json::to_string_pretty(&snippet).unwrap_or_default()
    )
}

/// Answer one Cline hook (`event` is the hook name, or
/// `UserPromptSubmit-capture`): the JSON Cline reads on stdout, `None` when
/// there is nothing to add (the script then prints `{"cancel":false}`).
pub async fn hook(event: &str, input: Value, env: &HookEnv) -> Result<Option<String>, String> {
    let context = match event {
        "TaskStart" | "TaskResume" => env.cards_text().await,
        "UserPromptSubmit" => Some(env.rules_reminder().await),
        PROMPT_CAPTURE => {
            if let Some(turn) = prompt_turn(&input) {
                env.record(vec![turn]).await;
            }
            Some(env.rules_reminder().await)
        }
        "PreToolUse" => env.files_context_text(&edited_files(&input)).await,
        "PostToolUse" => {
            if let Some(turn) = answer_turn(&input) {
                env.record(vec![turn]).await;
            }
            None
        }
        _ => None,
    };
    Ok(context.map(|text| reply(&text)))
}

/// Cline's hook reply carrying `text` as context.
fn reply(text: &str) -> String {
    json!({ "cancel": false, "contextModification": text }).to_string()
}

fn hook_script(ctx: &SetupContext, event: &str) -> String {
    format!(
        "#!/bin/sh\n{OWNED_MARK}; rerun setup to update it.\n\
         out=$({} 2>/dev/null)\n\
         if [ -n \"$out\" ]; then printf '%s\\n' \"$out\"; else echo '{{\"cancel\":false}}'; fi\n",
        ctx.hook_command(Client::Cline, event)
    )
}

/// Write a hook script unless a script of the user's has that name.
fn write_hook(path: &Path, script: &str, ctx: &SetupContext) -> Result<String, String> {
    if let Ok(existing) = std::fs::read_to_string(path)
        && !existing.contains(OWNED_MARK)
    {
        return Ok(skipped(&format!(
            "{} is not post-cortex's; to add memory, make it run `{}`",
            report_path(path, ctx),
            ctx.hook_command(
                Client::Cline,
                path.file_name().and_then(|n| n.to_str()).unwrap_or("")
            )
        )));
    }
    let change: Change = write_owned(path, script, true)?;
    Ok(report(change, path, ctx))
}

/// The files a `PreToolUse` edit is about to change (its `path`, else the
/// files of an `apply_patch` input; at most [`MAX_FILES`]), relative to
/// the workspace root when the payload gives them absolute. Empty for
/// tools that edit nothing.
fn edited_files(input: &Value) -> Vec<String> {
    let tool = &input["preToolUse"];
    if !tool["toolName"]
        .as_str()
        .is_some_and(|t| EDIT_TOOLS.contains(&t))
    {
        return Vec::new();
    }
    let params = &tool["parameters"];
    let paths = match params["path"].as_str() {
        Some(p) if !p.trim().is_empty() => vec![p.trim().to_string()],
        _ => params["input"]
            .as_str()
            .map(patch_paths)
            .unwrap_or_default(),
    };
    let root = input["workspaceRoots"][0].as_str();
    paths
        .iter()
        .map(|p| project_relative(p, root))
        .take(MAX_FILES)
        .collect()
}

fn prompt_turn(input: &Value) -> Option<Turn> {
    let text = input["userPromptSubmit"]["prompt"].as_str()?.trim();
    turn(input, "user", text, Vec::new())
}

/// The task's final answer, from a `PostToolUse` of a completion tool.
fn answer_turn(input: &Value) -> Option<Turn> {
    let tool = &input["postToolUse"];
    let name = tool["toolName"].as_str()?;
    if !ANSWER_TOOLS.contains(&name) {
        return None;
    }
    let text = ["result", "summary", "answer"]
        .iter()
        .find_map(|k| tool["parameters"][k].as_str())?
        .trim();
    turn(input, "assistant", text, vec![name.to_string()])
}

fn turn(input: &Value, role: &'static str, text: &str, tools: Vec<String>) -> Option<Turn> {
    if text.is_empty() {
        return None;
    }
    let task = input["taskId"].as_str().unwrap_or("cline");
    Some(Turn {
        turn_id: turn_id(task, role, text),
        role,
        text: text.to_string(),
        source_session: input["taskId"].as_str().map(str::to_string),
        tools,
        files: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
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
        for name in ["TaskStart", "TaskResume", "UserPromptSubmit", "PreToolUse"] {
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
        assert!(!hooks.join("PostToolUse").exists());

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
        let script =
            std::fs::read_to_string(dir.path().join(".clinerules/hooks/TaskStart")).unwrap();
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
    fn capture_adds_post_tool_use_and_prompt_capture() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx(dir.path(), true);
        install(&ctx).unwrap();
        let hooks = dir.path().join(".clinerules/hooks");
        let prompt = std::fs::read_to_string(hooks.join("UserPromptSubmit")).unwrap();
        assert!(prompt.contains(&ctx.hook_command(Client::Cline, PROMPT_CAPTURE)));
        assert!(hooks.join("PostToolUse").exists());
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
        let edit = json!({ "preToolUse": { "toolName": "write_to_file", "parameters": { "path": "a.rs" } } });
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
}
