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
//!   the file just read, once a session, so it is in context before the edit.
//! - `pre-edit` (BeforeTool on `write_file` and `replace`): BeforeTool cannot
//!   add context in Gemini CLI, so the first edit of a file the session was
//!   not told about is denied once with the memory as the reason (Gemini
//!   gives it to the agent as the tool's error); the same edit sent again
//!   goes through.
//! - `prompt` (BeforeAgent): the rules reminder.
//! - with capture, `capture` (AfterAgent): the prompt and the reply.
//!
//! Gemini runs project hooks only in a trusted folder.

use serde_json::{Value, json};

use crate::clients::Client;
use crate::clients::common::{
    FileMemory, FileMoment, HookEnv, McpEntry, REMOTE_TOKEN_NOTE, SetupContext, Turn,
    context_output, hook_arrays, install_skill, json_file, merge_json_report, project_relative,
    report, transport_patch, turn_id, upsert_block,
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
        (
            "BeforeTool",
            Some(EDIT_TOOLS),
            "pre-edit",
            "post-cortex-file-before-edit",
        ),
        ("BeforeAgent", None, "prompt", "post-cortex-rules"),
    ];
    if capture {
        hooks.push(("AfterAgent", None, "capture", "post-cortex-capture"));
    }
    hooks
}

/// Every event setup may write a hook for.
const EVENTS: [&str; 5] = [
    "SessionStart",
    "AfterTool",
    "BeforeTool",
    "BeforeAgent",
    "AfterAgent",
];

/// Gemini CLI's tools that change a file.
const EDIT_TOOLS: &str = "write_file|replace";

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

/// The file a `read_file`, `write_file` or `replace` call names, relative to
/// the session's `cwd` when given absolute.
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
                .file_memory(&[path], FileMoment::Read, input["session_id"].as_str())
                .await
                .map(|memory| match memory {
                    FileMemory::Added(text) | FileMemory::Held(text) => {
                        context_output("AfterTool", text)
                    }
                }))
        }
        "pre-edit" => {
            let Some(path) = read_path(&input) else {
                return Ok(None);
            };
            // BeforeTool cannot add context: only a held edit says anything.
            Ok(
                match env
                    .file_memory(&[path], FileMoment::Edit, input["session_id"].as_str())
                    .await
                {
                    Some(FileMemory::Held(reason)) => {
                        Some(json!({ "decision": "deny", "reason": reason }).to_string())
                    }
                    _ => None,
                },
            )
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
mod tests;
