// Copyright (c) 2026 Julius ML
// MIT License

//! Codex CLI: the MCP server in `.codex/config.toml`, the instructions block
//! in `AGENTS.md` (Codex does not read CLAUDE.md), hooks in
//! `.codex/hooks.json` and the skill in `.agents/skills`.
//!
//! Codex reads the project `.codex/` layer only for trusted projects, and
//! runs a hook only after the person trusted it in `/hooks`.
//!
//! Hooks (`pcx hook codex <event>`):
//! - `session-start` (SessionStart, also after compaction): the cards.
//! - `pre-edit` (PreToolUse on `apply_patch`): what the memory knows about
//!   each file the patch touches, once a session: the first patch of a file
//!   it knows is denied with that memory as the reason, and the same patch
//!   sent again goes through. Codex reads files through the shell, so there
//!   is no read to tell it on.
//! - `prompt` (UserPromptSubmit): the rules reminder.
//! - with capture, `capture-prompt` (UserPromptSubmit) stores the prompt and
//!   `stop` (Stop) the assistant's last message.

use serde_json::{Value, json};

use crate::clients::Client;
use crate::clients::common::{
    FileMoment, HookEnv, McpEntry, REMOTE_TOKEN_NOTE, SetupContext, Turn, context_output,
    file_memory_output, hook_arrays, install_skill, json_file, merge_json, patch_paths,
    project_relative, report, skipped, turn_id, upsert_block, upsert_toml_section,
};

const MCP_HEADER: &str = "[mcp_servers.post-cortex]";
/// Codex caps the AGENTS.md it reads at 32 KiB.
const AGENTS_MD_LIMIT: u64 = 32 * 1024;
/// Hook timeout in seconds.
const HOOK_TIMEOUT: u64 = 30;
/// Files a single patch gets memory for, at most.
const MAX_FILES: usize = 3;

/// The `[mcp_servers.post-cortex]` section: `command` and `args` (the pcx
/// stdio proxy) for a remote profile, else the daemon's URL.
fn mcp_section(ctx: &SetupContext) -> String {
    match ctx.mcp_entry(Client::Codex) {
        McpEntry::Stdio { command, args } => format!(
            "{MCP_HEADER}\ncommand = {}\nargs = {}\n",
            toml::Value::String(command),
            toml::Value::Array(args.into_iter().map(toml::Value::String).collect()),
        ),
        McpEntry::Http { url } => format!(
            "{MCP_HEADER}\nurl = {}\nbearer_token_env_var = \"PC_AUTH_TOKEN\"\nhttp_headers = {{ \"X-PCX-Author\" = {} }}\n",
            toml::Value::String(url),
            toml::Value::String(Client::Codex.author().to_string()),
        ),
    }
}

/// The hooks post-cortex installs, per Codex event: (event, matcher, pcx
/// hook event).
fn wanted_hooks(capture: bool) -> Vec<(&'static str, Option<&'static str>, &'static str)> {
    let mut hooks = vec![
        (
            "SessionStart",
            Some("startup|resume|compact"),
            "session-start",
        ),
        ("PreToolUse", Some("apply_patch|Edit|Write"), "pre-edit"),
        ("UserPromptSubmit", None, "prompt"),
    ];
    if capture {
        hooks.push(("UserPromptSubmit", None, "capture-prompt"));
        hooks.push(("Stop", None, "stop"));
    }
    hooks
}

/// Every Codex event post-cortex may have hooks under (so a re-run without
/// capture removes the capture hooks).
const EVENTS: [&str; 4] = ["SessionStart", "PreToolUse", "UserPromptSubmit", "Stop"];

/// The `hooks` object to merge into `.codex/hooks.json`: per event, the
/// person's own hook groups followed by ours (see [`hook_arrays`]).
fn hooks_patch(existing: &Value, ctx: &SetupContext) -> Value {
    let wanted = wanted_hooks(ctx.capture);
    let events = EVENTS
        .iter()
        .map(|event| {
            let ours = wanted
                .iter()
                .filter(|(e, ..)| e == event)
                .map(|(_, matcher, pcx_event)| {
                    let mut group = json!({
                        "hooks": [{
                            "type": "command",
                            "command": ctx.hook_command(Client::Codex, pcx_event),
                            "timeout": HOOK_TIMEOUT,
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
    json!({ "hooks": hook_arrays(&existing["hooks"], Client::Codex, events) })
}

/// Codex's own directory: `.codex` in the project, or for a global setup
/// `$CODEX_HOME` (default `~/.codex`; tests never read the variable).
fn codex_dir(ctx: &SetupContext) -> std::path::PathBuf {
    let home =
        std::env::var_os("CODEX_HOME").filter(|d| !cfg!(test) && ctx.global && !d.is_empty());
    match home {
        Some(dir) => dir.into(),
        None => ctx.project(".codex"),
    }
}

/// Write the Codex files: `.codex/config.toml`, `AGENTS.md`,
/// `.codex/hooks.json` and the skill in the project, or for a global setup
/// `~/.codex/config.toml`, `~/.codex/AGENTS.md`, `~/.codex/hooks.json` and
/// `~/.agents/skills`.
pub fn install(ctx: &SetupContext) -> Result<Vec<String>, String> {
    let mut lines = Vec::new();
    let dir = codex_dir(ctx);

    let config = dir.join("config.toml");
    let change = upsert_toml_section(&config, MCP_HEADER, &mcp_section(ctx))?;
    lines.push(if ctx.global {
        report(change, &config, ctx)
    } else {
        format!(
            "{} (read once the project is trusted in Codex)",
            report(change, &config, ctx)
        )
    });

    let agents = if ctx.global {
        dir.join("AGENTS.md")
    } else {
        ctx.project("AGENTS.md")
    };
    let change = upsert_block(&agents, &ctx.instructions())?;
    let mut line = report(change, &agents, ctx);
    if std::fs::metadata(&agents).is_ok_and(|m| m.len() > AGENTS_MD_LIMIT) {
        line.push_str(" (over 32 KiB: Codex reads only the first 32 KiB, keep the post-cortex block near the top)");
    }
    lines.push(line);

    let hooks = dir.join("hooks.json");
    lines.push(
        match merge_json(&hooks, &hooks_patch(&json_file(&hooks), ctx)) {
            Ok(change) => format!(
                "{} (review and trust the hooks with /hooks in Codex)",
                report(change, &hooks, ctx)
            ),
            Err(e) => skipped(&e),
        },
    );

    lines.extend(install_skill(&ctx.project(".agents/skills"), ctx)?);
    Ok(lines)
}

/// The `[mcp_servers.post-cortex]` section to paste.
pub fn mcp_config(ctx: &SetupContext) -> String {
    let note = if ctx.remote {
        REMOTE_TOKEN_NOTE
    } else {
        "The bearer token comes from PC_AUTH_TOKEN; drop that line if the daemon runs without auth."
    };
    format!(
        "# Add to .codex/config.toml in the project (trusted projects) or ~/.codex/config.toml.\n\
         # {note}\n{}",
        mcp_section(ctx)
    )
}

/// The files a PreToolUse payload is about to edit, relative to the
/// session's `cwd` when given absolute.
fn edited_paths(input: &Value) -> Vec<String> {
    let tool_input = &input["tool_input"];
    let mut paths = tool_input["command"]
        .as_str()
        .map(patch_paths)
        .unwrap_or_default();
    for key in ["file_path", "path"] {
        if let Some(p) = tool_input[key].as_str()
            && !paths.iter().any(|x| x == p)
        {
            paths.push(p.to_string());
        }
    }
    let cwd = input["cwd"].as_str();
    paths
        .iter()
        .map(|p| project_relative(p, cwd))
        .take(MAX_FILES)
        .collect()
}

/// The turn a capture hook stores: the user prompt (UserPromptSubmit) or
/// the assistant's last message (Stop). The id is the Codex turn id with
/// the role, else a hash of the text in the Codex session.
fn captured_turn(input: &Value, role: &'static str) -> Option<Turn> {
    let key = if role == "user" {
        "prompt"
    } else {
        "last_assistant_message"
    };
    let text = input[key].as_str()?.trim();
    if text.is_empty() {
        return None;
    }
    let session = input["session_id"].as_str().map(str::to_string);
    let turn_id = match input["turn_id"].as_str().filter(|t| !t.is_empty()) {
        Some(id) => format!("{id}:{role}"),
        None => turn_id(session.as_deref().unwrap_or("codex"), role, text),
    };
    Some(Turn {
        turn_id,
        role,
        text: text.to_string(),
        source_session: session,
        tools: Vec::new(),
        files: Vec::new(),
    })
}

/// Handle one Codex hook call (see the module docs for the events).
pub async fn hook(event: &str, input: Value, env: &HookEnv) -> Result<Option<String>, String> {
    match event {
        "session-start" => Ok(env
            .cards_text()
            .await
            .map(|text| context_output("SessionStart", text))),
        "pre-edit" => Ok(env
            .file_memory(
                &edited_paths(&input),
                FileMoment::Edit,
                input["session_id"].as_str(),
            )
            .await
            .map(|memory| file_memory_output("PreToolUse", memory))),
        "prompt" => Ok(Some(context_output(
            "UserPromptSubmit",
            env.rules_reminder().await,
        ))),
        "capture-prompt" | "stop" => {
            let role = if event == "stop" { "assistant" } else { "user" };
            if let Some(turn) = captured_turn(&input, role) {
                // An unreachable daemon must not hold up the turn.
                env.record(vec![turn]).await;
            }
            Ok(None)
        }
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests;
