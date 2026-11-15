// Copyright (c) 2026 Julius ML
// MIT License

//! Windsurf (Devin Desktop): MCP server, instructions and, with
//! `--capture`, the capture hooks.
//!
//! - MCP: Windsurf has no project MCP config, only the user-level
//!   `mcp_config.json`: `~/.config/devin/mcp_config.json` (Devin Desktop,
//!   `$XDG_CONFIG_HOME/devin` when set, `%APPDATA%\devin` on Windows) and the
//!   older `~/.codeium/windsurf/mcp_config.json`. Setup merges the entry into
//!   each of them whose directory exists, else into the Devin Desktop one.
//!   Headers take `${env:PC_AUTH_TOKEN}`.
//! - Instructions: the root `AGENTS.md` (always on in Cascade).
//! - Hooks (Cascade Hooks, `hooks.json`): no hook event can put text in
//!   front of the agent (stdout only shows in the UI, stderr reaches the
//!   agent only when a pre-hook blocks), so there are no cards, file memory
//!   or prompt reminder hooks. With `--capture`, `pre_user_prompt` and
//!   `post_cascade_response` send the turns to the session's message stream.
//!   They go into `.devin/hooks.json` when it exists, else into the legacy
//!   `.windsurf/hooks.json` (which Devin Desktop reads when `.devin/hooks.json`
//!   is absent), so existing hooks are never shadowed. A run without
//!   `--capture` takes post-cortex's capture hooks out of that file again.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::clients::Client;
use crate::clients::common::{
    HookEnv, McpEntry, SetupContext, Turn, hook_arrays, is_our_hook, json_file, merge_json,
    merge_json_report, report, skipped, transport_patch, turn_id, upsert_block, user_config_dir,
};

/// The hook events setup installs with `--capture`.
const CAPTURE_EVENTS: [&str; 2] = ["pre_user_prompt", "post_cascade_response"];

pub fn install(ctx: &SetupContext) -> Result<Vec<String>, String> {
    let home = dirs::home_dir().ok_or("Cannot find the home directory")?;
    let config = user_config_dir().ok_or("Cannot find the user config directory")?;
    install_at(ctx, &mcp_targets(&home, config.join("devin")))
}

/// `install` with the user-level MCP files given (tests point them into a
/// temp dir).
fn install_at(ctx: &SetupContext, mcp_files: &[PathBuf]) -> Result<Vec<String>, String> {
    let mut lines = Vec::new();
    let agents = ctx.project("AGENTS.md");
    lines.push(report(
        upsert_block(&agents, &ctx.instructions())?,
        &agents,
        ctx,
    ));

    let entry = json!({ "mcpServers": { "post-cortex": transport_patch(mcp_entry(ctx)) } });
    for path in mcp_files {
        lines.push(match merge_json(path, &entry) {
            Ok(change) => format!(
                "{} (user-level: Windsurf has no project MCP config)",
                report(change, path, ctx)
            ),
            Err(e) => skipped(&format!("{e}; `pcx mcp-config windsurf` prints the entry")),
        });
    }

    // Without capture, the file is touched only to take our hooks out.
    let path = hooks_file(ctx);
    let existing = json_file(&path);
    let ours_there = CAPTURE_EVENTS.iter().any(|event| {
        existing["hooks"][event]
            .as_array()
            .is_some_and(|list| list.iter().any(|h| is_our_hook(h, Client::Windsurf)))
    });
    if ctx.capture || ours_there {
        lines.push(merge_json_report(&path, &hooks_patch(&existing, ctx), ctx));
    }
    Ok(lines)
}

pub fn mcp_config(ctx: &SetupContext) -> String {
    let snippet = json!({ "mcpServers": { "post-cortex": mcp_entry(ctx) } });
    format!(
        "// Merge into ~/.config/devin/mcp_config.json (older Windsurf: ~/.codeium/windsurf/mcp_config.json)\n{}\n",
        serde_json::to_string_pretty(&snippet).unwrap_or_default()
    )
}

/// Capture only: `pre_user_prompt` stores the prompt, `post_cascade_response`
/// the answer. Cascade reads nothing from these hooks' stdout, so the reply
/// is always `None`.
pub async fn hook(event: &str, input: Value, env: &HookEnv) -> Result<Option<String>, String> {
    if let Some(turn) = capture_turn(event, &input) {
        // An unreachable daemon must not get in the way of the session.
        env.record(vec![turn]).await;
    }
    Ok(None)
}

/// The `post-cortex` entry: the pcx stdio proxy for a remote profile, else
/// the daemon over HTTP.
fn mcp_entry(ctx: &SetupContext) -> Value {
    match ctx.mcp_entry(Client::Windsurf) {
        McpEntry::Stdio { command, args } => json!({ "command": command, "args": args }),
        McpEntry::Http { url } => json!({
            "serverUrl": url,
            "headers": {
                "Authorization": "Bearer ${env:PC_AUTH_TOKEN}",
                "X-PCX-Author": Client::Windsurf.author(),
            }
        }),
    }
}

/// The user-level MCP files to write: each one whose directory exists
/// (Devin Desktop, older Windsurf), else the Devin Desktop one.
fn mcp_targets(home: &Path, devin_dir: PathBuf) -> Vec<PathBuf> {
    let candidates = [devin_dir, home.join(".codeium").join("windsurf")];
    let existing: Vec<PathBuf> = candidates
        .iter()
        .filter(|dir| dir.is_dir())
        .map(|dir| dir.join("mcp_config.json"))
        .collect();
    if existing.is_empty() {
        vec![candidates[0].join("mcp_config.json")]
    } else {
        existing
    }
}

/// `.devin/hooks.json` when present (it shadows the legacy file), else
/// `.windsurf/hooks.json`.
fn hooks_file(ctx: &SetupContext) -> PathBuf {
    let devin = ctx.project(".devin/hooks.json");
    if devin.exists() {
        devin
    } else {
        ctx.project(".windsurf/hooks.json")
    }
}

/// The hooks.json patch: per capture event, the user's own hooks, then
/// ours when capture is on (see [`hook_arrays`]).
fn hooks_patch(existing: &Value, ctx: &SetupContext) -> Value {
    let events = CAPTURE_EVENTS
        .iter()
        .map(|event| {
            let ours = if ctx.capture {
                vec![json!({ "command": ctx.hook_command(Client::Windsurf, event) })]
            } else {
                Vec::new()
            };
            (*event, ours)
        })
        .collect();
    json!({ "hooks": hook_arrays(&existing["hooks"], Client::Windsurf, events) })
}

/// The turn a capture hook payload carries: the prompt (`pre_user_prompt`)
/// or the answer since the last prompt (`post_cascade_response`).
fn capture_turn(event: &str, input: &Value) -> Option<Turn> {
    let (role, field) = match event {
        "pre_user_prompt" => ("user", "user_prompt"),
        "post_cascade_response" => ("assistant", "response"),
        _ => return None,
    };
    let text = input["tool_info"][field].as_str()?.trim().to_string();
    if text.is_empty() {
        return None;
    }
    let conversation = input["trajectory_id"].as_str().unwrap_or("windsurf");
    Some(Turn {
        turn_id: turn_id(conversation, role, &text),
        role,
        text,
        source_session: input["trajectory_id"].as_str().map(str::to_string),
        tools: Vec::new(),
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

    fn read_json(path: &Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn install_writes_agents_md_and_user_mcp_and_is_idempotent() {
        let project = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let mcp = home.path().join("devin/mcp_config.json");
        let ctx = ctx(project.path(), false);

        let lines = install_at(&ctx, std::slice::from_ref(&mcp)).unwrap();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("Created: AGENTS.md"));
        assert!(lines[1].contains("user-level"));
        let v = read_json(&mcp);
        let entry = &v["mcpServers"]["post-cortex"];
        assert_eq!(entry["serverUrl"], "http://127.0.0.1:3737/mcp");
        assert_eq!(entry["headers"]["X-PCX-Author"], "windsurf");
        assert_eq!(
            entry["headers"]["Authorization"],
            "Bearer ${env:PC_AUTH_TOKEN}"
        );
        let agents = std::fs::read_to_string(project.path().join("AGENTS.md")).unwrap();
        assert!(agents.contains("S1") && agents.contains("W1"));
        assert!(!project.path().join(".windsurf").exists());

        let before = std::fs::read(&mcp).unwrap();
        let again = install_at(&ctx, std::slice::from_ref(&mcp)).unwrap();
        assert!(
            again.iter().all(|l| l.contains("already set up")),
            "{again:?}"
        );
        assert_eq!(std::fs::read(&mcp).unwrap(), before);
    }

    #[test]
    fn install_keeps_other_mcp_servers_and_skips_jsonc() {
        let project = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let mcp = home.path().join("mcp_config.json");
        std::fs::write(&mcp, r#"{"mcpServers":{"other":{"command":"x"}}}"#).unwrap();
        install_at(&ctx(project.path(), false), std::slice::from_ref(&mcp)).unwrap();
        let v = read_json(&mcp);
        assert_eq!(v["mcpServers"]["other"]["command"], "x");
        assert!(v["mcpServers"]["post-cortex"].is_object());

        std::fs::write(&mcp, "{ // mine\n}").unwrap();
        let lines = install_at(&ctx(project.path(), false), std::slice::from_ref(&mcp)).unwrap();
        assert!(lines[1].contains("Skipped") && lines[1].contains("pcx mcp-config windsurf"));
        assert_eq!(std::fs::read_to_string(&mcp).unwrap(), "{ // mine\n}");
    }

    #[test]
    fn remote_install_writes_the_stdio_entry_and_no_token() {
        let project = tempfile::tempdir().unwrap();
        let mcp = project.path().join("mcp_config.json");
        install_at(&ctx(project.path(), true), std::slice::from_ref(&mcp)).unwrap();
        install_at(
            &remote(ctx(project.path(), true)),
            std::slice::from_ref(&mcp),
        )
        .unwrap();
        let text = std::fs::read_to_string(&mcp).unwrap();
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            v["mcpServers"]["post-cortex"],
            json!({ "command": "pcx", "args": cloud_args(Client::Windsurf) })
        );
        assert!(!text.contains("Bearer"), "{text}");
        let hooks = std::fs::read_to_string(hooks_file(&ctx(project.path(), true))).unwrap();
        assert!(
            hooks.contains("--profile cloud") && !hooks.contains("--url"),
            "{hooks}"
        );
    }

    #[test]
    fn mcp_targets_prefer_existing_dirs() {
        let home = tempfile::tempdir().unwrap();
        let devin = home.path().join(".config/devin");
        assert_eq!(
            mcp_targets(home.path(), devin.clone()),
            vec![devin.join("mcp_config.json")]
        );
        let legacy = home.path().join(".codeium/windsurf");
        std::fs::create_dir_all(&legacy).unwrap();
        assert_eq!(
            mcp_targets(home.path(), devin.clone()),
            vec![legacy.join("mcp_config.json")]
        );
        std::fs::create_dir_all(&devin).unwrap();
        assert_eq!(mcp_targets(home.path(), devin.clone()).len(), 2);
    }

    #[test]
    fn capture_hooks_merge_into_existing_hooks() {
        let project = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let mcp = home.path().join("mcp_config.json");
        let hooks = project.path().join(".windsurf/hooks.json");
        std::fs::create_dir_all(hooks.parent().unwrap()).unwrap();
        std::fs::write(
            &hooks,
            r#"{"hooks":{"post_cascade_response":[{"command":"mine.sh"}],"pre_run_command":[{"command":"guard"}]}}"#,
        )
        .unwrap();
        let ctx = ctx(project.path(), true);
        let lines = install_at(&ctx, std::slice::from_ref(&mcp)).unwrap();
        assert!(
            lines[2].contains("Updated: .windsurf/hooks.json"),
            "{lines:?}"
        );
        let v = read_json(&hooks);
        assert_eq!(v["hooks"]["pre_run_command"][0]["command"], "guard");
        let post = v["hooks"]["post_cascade_response"].as_array().unwrap();
        assert_eq!(post.len(), 2);
        assert_eq!(post[0]["command"], "mine.sh");
        assert_eq!(
            post[1]["command"],
            ctx.hook_command(Client::Windsurf, "post_cascade_response")
        );
        assert_eq!(v["hooks"]["pre_user_prompt"].as_array().unwrap().len(), 1);

        let before = std::fs::read(&hooks).unwrap();
        let again = install_at(&ctx, std::slice::from_ref(&mcp)).unwrap();
        assert!(again[2].contains("already set up"));
        assert_eq!(std::fs::read(&hooks).unwrap(), before);
    }

    #[test]
    fn capture_hooks_go_into_devin_file_when_present() {
        let project = tempfile::tempdir().unwrap();
        let devin = project.path().join(".devin/hooks.json");
        std::fs::create_dir_all(devin.parent().unwrap()).unwrap();
        std::fs::write(&devin, "{}").unwrap();
        let ctx = ctx(project.path(), true);
        assert_eq!(hooks_file(&ctx), devin);
        let mcp = project.path().join("mcp_config.json");
        install_at(&ctx, std::slice::from_ref(&mcp)).unwrap();
        assert!(read_json(&devin)["hooks"]["pre_user_prompt"].is_array());
        assert!(!project.path().join(".windsurf").exists());
    }

    #[test]
    fn capture_off_removes_our_hooks_and_leaves_other_files_alone() {
        let project = tempfile::tempdir().unwrap();
        let mcp = project.path().join("mcp_config.json");
        let hooks = project.path().join(".windsurf/hooks.json");
        std::fs::create_dir_all(hooks.parent().unwrap()).unwrap();
        // An entry an older setup wrote, with plain `pcx`.
        std::fs::write(
            &hooks,
            r#"{"hooks":{"pre_user_prompt":[{"command":"mine.sh"},{"command":"pcx hook windsurf pre_user_prompt --session OLD"}]}}"#,
        )
        .unwrap();
        let lines = install_at(&ctx(project.path(), false), std::slice::from_ref(&mcp)).unwrap();
        assert!(
            lines[2].contains("Updated: .windsurf/hooks.json"),
            "{lines:?}"
        );
        let v = read_json(&hooks);
        assert_eq!(
            v["hooks"]["pre_user_prompt"],
            json!([{ "command": "mine.sh" }])
        );
        assert!(v["hooks"].get("post_cascade_response").is_none());

        // Nothing of ours left: the file is not touched or reported.
        let lines = install_at(&ctx(project.path(), false), std::slice::from_ref(&mcp)).unwrap();
        assert_eq!(lines.len(), 2, "{lines:?}");
    }

    #[test]
    fn capture_turn_parses_payloads() {
        let prompt = json!({
            "agent_action_name": "pre_user_prompt",
            "trajectory_id": "T1",
            "tool_info": { "user_prompt": " fix the build " }
        });
        let turn = capture_turn("pre_user_prompt", &prompt).unwrap();
        assert_eq!(turn.role, "user");
        assert_eq!(turn.text, "fix the build");
        assert_eq!(turn.source_session.as_deref(), Some("T1"));
        assert_eq!(turn.turn_id, turn_id("T1", "user", "fix the build"));

        let answer = json!({ "trajectory_id": "T1", "tool_info": { "response": "Done." } });
        let turn = capture_turn("post_cascade_response", &answer).unwrap();
        assert_eq!(turn.role, "assistant");
        assert_eq!(turn.text, "Done.");

        assert!(capture_turn("pre_write_code", &answer).is_none());
        assert!(capture_turn("pre_user_prompt", &json!({"tool_info": {}})).is_none());
    }

    #[tokio::test]
    async fn hook_prints_nothing_even_when_daemon_is_down() {
        let env = HookEnv::new(
            "http://127.0.0.1:9",
            reqwest::Client::new(),
            "S1",
            "W1",
            "windsurf",
        );
        let answer = json!({ "trajectory_id": "T1", "tool_info": { "response": "Done." } });
        for event in [
            "post_cascade_response",
            "pre_user_prompt",
            "pre_write_code",
            "x",
        ] {
            assert_eq!(hook(event, answer.clone(), &env).await, Ok(None));
        }
    }

    #[test]
    fn mcp_config_names_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let text = mcp_config(&ctx(dir.path(), false));
        assert!(text.starts_with("// Merge into ~/.config/devin/mcp_config.json"));
        let json_part = text.split_once('\n').unwrap().1;
        let v: Value = serde_json::from_str(json_part).unwrap();
        assert_eq!(
            v["mcpServers"]["post-cortex"]["headers"]["X-PCX-Author"],
            "windsurf"
        );
    }
}
