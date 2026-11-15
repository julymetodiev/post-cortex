// Copyright (c) 2026 Julius ML
// MIT License

//! Cursor (editor and CLI).
//!
//! `install` writes, all inside the project:
//! - `.cursor/mcp.json`: the `post-cortex` server (merged into the file).
//! - `.cursor/rules/post-cortex.mdc`: the shared instructions as an
//!   always-applied rule. Cursor also reads AGENTS.md; it is not written so
//!   the rules are not in front of the agent twice.
//! - `.cursor/hooks.json`: `sessionStart` (the cards) and `postToolUse` on
//!   `Read` (what the memory knows about the file just read, so it is in
//!   context before the edit; `preToolUse` cannot add context). With
//!   capture, `beforeSubmitPrompt` (the prompt) and `afterAgentResponse`
//!   (the reply). The user's own hooks for the same events are kept.
//! - `.agents/skills/post-cortex-memory`: the skill, unless a copy is
//!   already in a directory Cursor reads skills from.

use serde_json::{Value, json};

use crate::clients::Client;
use crate::clients::common::{
    Change, HookEnv, McpEntry, REMOTE_TOKEN_NOTE, SetupContext, Turn, hook_arrays, install_skill,
    json_file, merge_json_report, project_relative, report, transport_patch, turn_id, urlencode,
    write_owned,
};

const MCP_JSON: &str = ".cursor/mcp.json";
const RULE_FILE: &str = ".cursor/rules/post-cortex.mdc";
const HOOKS_JSON: &str = ".cursor/hooks.json";
/// Project skill directories Cursor reads, the first being where the skill
/// is installed.
const SKILL_DIRS: [&str; 3] = [".agents/skills", ".cursor/skills", ".claude/skills"];

/// The hook events post-cortex manages, with the tool matcher (if any) and
/// whether the event is a capture hook.
const HOOK_EVENTS: [(&str, Option<&str>, bool); 4] = [
    ("sessionStart", None, false),
    ("postToolUse", Some("Read"), false),
    ("beforeSubmitPrompt", None, true),
    ("afterAgentResponse", None, true),
];

/// Write Cursor's project files (see the module docs), or for a global
/// setup the user-level `~/.cursor/mcp.json`, `~/.cursor/hooks.json` and
/// `~/.cursor/skills`. Cursor has no user-level rules file, so a global
/// setup writes no rule: the session-start hook names the project's
/// session and workspace and the rules.
pub fn install(ctx: &SetupContext) -> Result<Vec<String>, String> {
    let mut lines = Vec::new();

    let mcp = ctx.project(MCP_JSON);
    lines.push(merge_json_report(
        &mcp,
        &json!({ "mcpServers": { "post-cortex": transport_patch(server_entry(ctx)) } }),
        ctx,
    ));

    if !ctx.global {
        let rule = ctx.project(RULE_FILE);
        let change = write_owned(&rule, &rule_text(ctx), false)?;
        lines.push(report(change, &rule, ctx));
    }

    let hooks = ctx.project(HOOKS_JSON);
    lines.push(merge_json_report(
        &hooks,
        &hooks_patch(&json_file(&hooks), ctx),
        ctx,
    ));

    match SKILL_DIRS
        .iter()
        .map(|dir| ctx.project(dir).join("post-cortex-memory"))
        .find(|skill| skill.join("SKILL.md").exists())
    {
        Some(skill) => lines.push(report(Change::Unchanged, &skill, ctx)),
        None => {
            let dir = if ctx.global {
                SKILL_DIRS[1]
            } else {
                SKILL_DIRS[0]
            };
            lines.extend(install_skill(&ctx.project(dir), ctx)?)
        }
    }
    Ok(lines)
}

/// The `.cursor/mcp.json` entry and the one-click install link.
pub fn mcp_config(ctx: &SetupContext) -> String {
    let server = server_entry(ctx);
    let file = json!({ "mcpServers": { "post-cortex": server } });
    let note = if ctx.remote {
        REMOTE_TOKEN_NOTE
    } else {
        "PC_AUTH_TOKEN is read from the environment Cursor runs in."
    };
    format!(
        "# Add to .cursor/mcp.json (this project) or ~/.cursor/mcp.json (every project).\n\
         # {note}\n\
         {}\n\n\
         # Or install with one click (open this link):\n\
         {}\n",
        serde_json::to_string_pretty(&file).unwrap_or_default(),
        install_link(&server)
    )
}

/// Handle a Cursor hook: `event` is Cursor's event name, `input` its stdin
/// payload. Prints `{"additional_context": ...}` where Cursor injects
/// context, `{"continue": true}` for `beforeSubmitPrompt`, nothing else.
pub async fn hook(event: &str, input: Value, env: &HookEnv) -> Result<Option<String>, String> {
    match event {
        "sessionStart" => Ok(env.cards_text().await.map(additional_context)),
        "postToolUse" => {
            let Some(path) = read_path(&input) else {
                return Ok(None);
            };
            Ok(env.file_context_text(&path).await.map(additional_context))
        }
        "beforeSubmitPrompt" => {
            if let Some(turn) = prompt_turn(&input)
                && let Err(e) = env.capture(vec![turn]).await
            {
                eprintln!("pcx hook cursor: capture failed: {e}");
            }
            // The prompt goes on whatever capture did.
            Ok(Some(json!({ "continue": true }).to_string()))
        }
        "afterAgentResponse" => {
            if let Some(turn) = response_turn(&input)
                && let Err(e) = env.capture(vec![turn]).await
            {
                eprintln!("pcx hook cursor: capture failed: {e}");
            }
            Ok(None)
        }
        _ => Ok(None),
    }
}

// ------------------------------------------------------------ install side

/// The `post-cortex` server object: the pcx stdio proxy for a remote
/// profile, else the daemon over HTTP (Cursor interpolates `${env:..}` in
/// headers).
fn server_entry(ctx: &SetupContext) -> Value {
    match ctx.mcp_entry(Client::Cursor) {
        McpEntry::Stdio { command, args } => json!({ "command": command, "args": args }),
        McpEntry::Http { url } => json!({
            "url": url,
            "headers": {
                "Authorization": "Bearer ${env:PC_AUTH_TOKEN}",
                "X-PCX-Author": Client::Cursor.author(),
            }
        }),
    }
}

/// `cursor://anysphere.cursor-deeplink/mcp/install?name=..&config=..`, the
/// config being the base64 of the server object's JSON.
fn install_link(server: &Value) -> String {
    let config = base64(serde_json::to_string(server).unwrap_or_default().as_bytes());
    format!(
        "cursor://anysphere.cursor-deeplink/mcp/install?name=post-cortex&config={}",
        urlencode(&config)
    )
}

fn rule_text(ctx: &SetupContext) -> String {
    format!(
        "---\ndescription: post-cortex memory rules\nalwaysApply: true\n---\n\n{}\n",
        ctx.instructions().trim_end()
    )
}

/// The hooks.json patch: `version` and, per managed event, the user's
/// entries plus ours when the event is installed (see [`hook_arrays`]).
fn hooks_patch(existing: &Value, ctx: &SetupContext) -> Value {
    let events = HOOK_EVENTS
        .iter()
        .map(|&(event, matcher, capture)| {
            let mut ours = Vec::new();
            if !capture || ctx.capture {
                let mut entry = json!({ "command": ctx.hook_command(Client::Cursor, event) });
                if let Some(m) = matcher {
                    entry["matcher"] = json!(m);
                }
                ours.push(entry);
            }
            (event, ours)
        })
        .collect();
    json!({ "version": 1, "hooks": hook_arrays(&existing["hooks"], Client::Cursor, events) })
}

/// Standard base64 with padding.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

// --------------------------------------------------------------- hook side

fn additional_context(text: String) -> String {
    json!({ "additional_context": text }).to_string()
}

/// The file a `postToolUse` payload read, relative to the workspace root
/// when absolute; `None` for other tools.
fn read_path(input: &Value) -> Option<String> {
    if !input["tool_name"]
        .as_str()
        .is_some_and(|t| t.eq_ignore_ascii_case("read"))
    {
        return None;
    }
    let args = &input["tool_input"];
    let path = ["file_path", "path", "target_file"]
        .iter()
        .find_map(|key| args[*key].as_str())
        .filter(|p| !p.trim().is_empty())?;
    Some(relative(path, input))
}

/// `path` relative to `workspace_roots[0]` (else `cwd`) when inside it.
fn relative(path: &str, input: &Value) -> String {
    let root = input["workspace_roots"][0]
        .as_str()
        .or_else(|| input["cwd"].as_str());
    let rel = project_relative(path, root);
    rel.strip_prefix("./").unwrap_or(&rel).to_string()
}

/// The user turn of a `beforeSubmitPrompt` payload.
fn prompt_turn(input: &Value) -> Option<Turn> {
    turn(input, "user", input["prompt"].as_str()?)
}

/// The assistant turn of an `afterAgentResponse` payload.
fn response_turn(input: &Value) -> Option<Turn> {
    turn(input, "assistant", input["text"].as_str()?)
}

/// A turn keyed on the conversation and generation ids (a prompt and its
/// reply share the generation), else on a hash of the text.
fn turn(input: &Value, role: &'static str, text: &str) -> Option<Turn> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let conversation = input["conversation_id"].as_str().filter(|c| !c.is_empty());
    let id = match (conversation, input["generation_id"].as_str()) {
        (Some(c), Some(g)) if !g.is_empty() => format!("{c}:{g}:{role}"),
        (c, _) => turn_id(c.unwrap_or("cursor"), role, text),
    };
    Some(Turn {
        turn_id: id,
        role,
        text: text.to_string(),
        source_session: conversation.map(str::to_string),
        tools: Vec::new(),
        files: Vec::new(),
    })
}

#[cfg(test)]
mod tests;
