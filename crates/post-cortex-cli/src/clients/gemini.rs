// Copyright (c) 2026 Julius ML
// MIT License

//! Gemini CLI.
//!
//! `install` writes, all inside the project:
//! - `.gemini/settings.json` (merged into the file): the `post-cortex` MCP
//!   server (`httpUrl`; the bearer token is `${PC_AUTH_TOKEN:-}`, which
//!   Gemini CLI expands in settings values), `context.fileName` with
//!   `AGENTS.md` added (Gemini reads only GEMINI.md by default) and the
//!   hooks. The user's own servers, context files and hooks are kept.
//! - `AGENTS.md`: the shared instructions block.
//! - `.agents/skills/post-cortex-memory`: the skill (Gemini reads workspace
//!   skills from `.gemini/skills` and its `.agents/skills` alias).
//!
//! Hooks (`pcx hook gemini <event>`):
//! - `session-start` (SessionStart): the cards.
//! - `file-context` (AfterTool on `read_file`): what the memory knows about
//!   the file just read, so it is in context before the edit. BeforeTool
//!   cannot add context in Gemini CLI, so the read is where it goes.
//! - `prompt` (BeforeAgent): the rules reminder.
//! - with capture, `capture` (AfterAgent): the prompt and the reply.
//!
//! Gemini runs project hooks only in a trusted folder.

use serde_json::{Value, json};

use crate::clients::Client;
use crate::clients::common::{
    HookEnv, McpEntry, REMOTE_TOKEN_NOTE, SetupContext, Turn, context_output, hook_arrays,
    install_skill, json_file, merge_json_report, project_relative, report, transport_patch,
    turn_id, upsert_block,
};

const SETTINGS: &str = ".gemini/settings.json";
/// Gemini's default context file, kept in the list next to AGENTS.md.
const DEFAULT_CONTEXT: &str = "GEMINI.md";
/// Hook timeout, in milliseconds.
const TIMEOUT_MS: u64 = 10_000;

/// The `post-cortex` entry for `mcpServers`: the pcx stdio proxy for a
/// remote profile, else the daemon over HTTP.
fn server_entry(ctx: &SetupContext) -> Value {
    match ctx.mcp_entry(Client::Gemini) {
        McpEntry::Stdio { command, args } => json!({ "command": command, "args": args }),
        McpEntry::Http { url } => json!({
            "httpUrl": url,
            "headers": {
                "X-PCX-Author": Client::Gemini.author(),
                "Authorization": "Bearer ${PC_AUTH_TOKEN:-}",
            }
        }),
    }
}

/// The hook groups setup installs: (event, matcher, `pcx hook` event, name).
fn wanted_hooks(
    capture: bool,
) -> Vec<(
    &'static str,
    Option<&'static str>,
    &'static str,
    &'static str,
)> {
    let mut hooks = vec![
        ("SessionStart", None, "session-start", "post-cortex-cards"),
        (
            "AfterTool",
            Some("read_file"),
            "file-context",
            "post-cortex-file-context",
        ),
        ("BeforeAgent", None, "prompt", "post-cortex-rules"),
    ];
    if capture {
        hooks.push(("AfterAgent", None, "capture", "post-cortex-capture"));
    }
    hooks
}

/// Every event setup may write a hook for.
const EVENTS: [&str; 4] = ["SessionStart", "AfterTool", "BeforeAgent", "AfterAgent"];

/// The `hooks` patch: per event, the user's groups, then ours (an earlier
/// copy of ours is replaced, so a changed session or a capture turned off
/// is picked up; see [`hook_arrays`]).
fn hooks_patch(existing: &Value, ctx: &SetupContext) -> Value {
    let wanted = wanted_hooks(ctx.capture);
    let events = EVENTS
        .iter()
        .map(|event| {
            let ours = wanted
                .iter()
                .filter(|(e, ..)| e == event)
                .map(|(_, matcher, pcx_event, name)| {
                    let mut group = json!({
                        "hooks": [{
                            "type": "command",
                            "name": name,
                            "command": ctx.hook_command(Client::Gemini, pcx_event),
                            "timeout": TIMEOUT_MS,
                        }]
                    });
                    if let Some(matcher) = matcher {
                        group["matcher"] = json!(matcher);
                    }
                    group
                })
                .collect();
            (*event, ours)
        })
        .collect();
    hook_arrays(existing, Client::Gemini, events)
}

/// `context.fileName` with AGENTS.md in it: the user's list (a string or
/// an array) with AGENTS.md added, or AGENTS.md and GEMINI.md when unset.
/// `None` when AGENTS.md is already listed.
fn context_files(existing: &Value) -> Option<Value> {
    let mut names: Vec<Value> = match existing {
        Value::String(name) => vec![json!(name)],
        Value::Array(names) => names.clone(),
        _ => vec![json!(DEFAULT_CONTEXT)],
    };
    if names.iter().any(|n| n == "AGENTS.md") {
        return None;
    }
    names.insert(0, json!("AGENTS.md"));
    Some(Value::Array(names))
}

/// The patch `install` merges into `.gemini/settings.json`.
fn settings_patch(existing: &Value, ctx: &SetupContext) -> Value {
    let mut patch = json!({
        "mcpServers": { "post-cortex": transport_patch(server_entry(ctx)) },
        "hooks": hooks_patch(&existing["hooks"], ctx),
    });
    if let Some(names) = context_files(&existing["context"]["fileName"]) {
        patch["context"] = json!({ "fileName": names });
    }
    patch
}

/// Set up Gemini CLI in the project (see the module docs), or for a
/// global setup in `~/.gemini/settings.json`, `~/.gemini/AGENTS.md` (found
/// through `context.fileName`) and `~/.agents/skills`.
pub fn install(ctx: &SetupContext) -> Result<Vec<String>, String> {
    let settings = ctx.project(SETTINGS);
    let patch = settings_patch(&json_file(&settings), ctx);
    let mut lines = vec![merge_json_report(&settings, &patch, ctx)];
    let agents = if ctx.global {
        ctx.project(".gemini/AGENTS.md")
    } else {
        ctx.project("AGENTS.md")
    };
    lines.push(report(
        upsert_block(&agents, &ctx.instructions())?,
        &agents,
        ctx,
    ));
    lines.extend(install_skill(&ctx.project(".agents/skills"), ctx)?);
    Ok(lines)
}

/// The MCP entry to paste into `.gemini/settings.json`.
pub fn mcp_config(ctx: &SetupContext) -> String {
    let snippet = json!({ "mcpServers": { "post-cortex": server_entry(ctx) } });
    let note = if ctx.remote {
        REMOTE_TOKEN_NOTE
    } else {
        "The bearer token comes from PC_AUTH_TOKEN (Gemini expands ${VAR} in settings); empty when unset."
    };
    format!(
        "# Merge into .gemini/settings.json (project) or ~/.gemini/settings.json (user).\n\
         # {note}\n{}\n",
        serde_json::to_string_pretty(&snippet).unwrap_or_default()
    )
}

/// The file a `read_file` call read, relative to the session's `cwd` when
/// given absolute.
fn read_path(input: &Value) -> Option<String> {
    let path = input["tool_input"]["file_path"].as_str()?.trim();
    if path.is_empty() {
        return None;
    }
    Some(project_relative(path, input["cwd"].as_str()))
}

/// The turns an AfterAgent payload carries: the prompt and the reply.
fn captured_turns(input: &Value) -> Vec<Turn> {
    let session = input["session_id"].as_str().map(str::to_string);
    let conversation = session.clone().unwrap_or_else(|| "gemini".to_string());
    [("user", "prompt"), ("assistant", "prompt_response")]
        .into_iter()
        .filter_map(|(role, key)| {
            let text = input[key].as_str()?.trim();
            (!text.is_empty()).then(|| Turn {
                turn_id: turn_id(&conversation, role, text),
                role,
                text: text.to_string(),
                source_session: session.clone(),
                tools: Vec::new(),
                files: Vec::new(),
            })
        })
        .collect()
}

/// Handle one Gemini CLI hook call (see the module docs for the events).
pub async fn hook(event: &str, input: Value, env: &HookEnv) -> Result<Option<String>, String> {
    match event {
        "session-start" => Ok(env
            .cards_text()
            .await
            .map(|text| context_output("SessionStart", text))),
        "file-context" => {
            let Some(path) = read_path(&input) else {
                return Ok(None);
            };
            Ok(env
                .file_context_text(&path)
                .await
                .map(|text| context_output("AfterTool", text)))
        }
        "prompt" => Ok(Some(context_output(
            "BeforeAgent",
            env.rules_reminder().await,
        ))),
        "capture" => {
            // An unreachable daemon must not hold up the turn.
            env.record(captured_turns(&input)).await;
            Ok(None)
        }
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
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
}
