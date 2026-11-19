// Copyright (c) 2026 Julius ML
// MIT License

//! VS Code with GitHub Copilot agent mode.
//!
//! `install` writes, all inside the project:
//! - `.vscode/mcp.json` (merged into the file): the `post-cortex` server and
//!   the `pc-auth-token` input VS Code asks for once (and keeps in its
//!   secret storage) for the bearer token; leave it empty when the daemon
//!   runs without auth. The user's own servers and inputs are kept. A file
//!   with comments is refused; `pcx mcp-config vscode` prints the entry.
//! - `AGENTS.md`: the shared instructions block (Copilot reads it).
//! - `.github/hooks/post-cortex.json`: the hooks (VS Code's own hook
//!   format, read from `.github/hooks/*.json` in a trusted workspace).
//! - `.agents/skills/post-cortex-memory`: the skill (VS Code reads skills
//!   from `.github/skills`, `.claude/skills` and `.agents/skills`).
//!
//! Hooks (`pcx hook vscode <event>`):
//! - `session-start` (SessionStart): the cards.
//! - `post-read` (PostToolUse): what the memory knows about the file a read
//!   tool returned, once a session, with the read.
//! - `pre-edit` (PreToolUse): the same for a file an edit tool is about to
//!   change and the session was not told about: the edit is denied once with
//!   it as the reason, and the same edit sent again goes through.
//!   VS Code has no tool matchers in this format, so both hooks run for
//!   every tool and ignore the ones that read or edit nothing.
//! - with capture, `capture-prompt` (UserPromptSubmit) stores the prompt and
//!   `stop` (Stop) the assistant's last reply, read from the transcript.
//!
//! There is no prompt reminder: UserPromptSubmit cannot add context in VS
//! Code. VS Code also runs the Claude Code hooks in `.claude/settings.json`
//! when `chat.useClaudeHooks` is on (off by default); a project set up for
//! both then gets the cards and the file memory twice.

use serde_json::{Value, json};

use crate::clients::Client;
use crate::clients::common::{
    FileMoment, HookEnv, McpEntry, REMOTE_TOKEN_NOTE, SetupContext, Turn, context_output,
    file_memory_output, install_skill, json_file, merge_json_report, patch_paths, project_relative,
    report, transport_patch, turn_id, upsert_block, write_owned,
};

const MCP: &str = ".vscode/mcp.json";
const HOOKS: &str = ".github/hooks/post-cortex.json";
const INPUT_ID: &str = "pc-auth-token";
/// Hook timeout, in seconds.
const TIMEOUT_S: u64 = 10;
/// At most this many files get their memory per edit.
const MAX_FILES: usize = 3;

/// The `post-cortex` entry for `servers`: the pcx stdio proxy for a
/// remote profile, else the daemon over HTTP.
fn server_entry(ctx: &SetupContext) -> Value {
    match ctx.mcp_entry(Client::Vscode) {
        McpEntry::Stdio { command, args } => {
            json!({ "type": "stdio", "command": command, "args": args })
        }
        McpEntry::Http { url } => json!({
            "type": "http",
            "url": url,
            "headers": {
                "X-PCX-Author": Client::Vscode.author(),
                "Authorization": format!("Bearer ${{input:{INPUT_ID}}}"),
            }
        }),
    }
}

/// The input VS Code prompts for the bearer token.
fn token_input() -> Value {
    json!({
        "type": "promptString",
        "id": INPUT_ID,
        "description": "post-cortex token (empty if none)",
        "password": true,
    })
}

/// The patch `install` merges into `.vscode/mcp.json`: the server, and
/// the user's inputs with ours added (or refreshed; taken out for a remote
/// profile, whose stdio entry needs no token).
fn mcp_patch(existing: &Value, ctx: &SetupContext) -> Value {
    let mut inputs: Vec<Value> = existing["inputs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|i| i["id"] != INPUT_ID)
        .cloned()
        .collect();
    if !ctx.remote {
        inputs.push(token_input());
    }
    json!({
        "servers": { "post-cortex": transport_patch(server_entry(ctx)) },
        "inputs": inputs,
    })
}

/// `.github/hooks/post-cortex.json`.
fn hooks_file(ctx: &SetupContext) -> String {
    let entry = |event: &str| {
        json!([{
            "type": "command",
            "command": ctx.hook_command(Client::Vscode, event),
            "timeout": TIMEOUT_S,
        }])
    };
    let mut hooks = json!({
        "SessionStart": entry("session-start"),
        "PreToolUse": entry("pre-edit"),
        "PostToolUse": entry("post-read"),
    });
    if ctx.capture {
        hooks["UserPromptSubmit"] = entry("capture-prompt");
        hooks["Stop"] = entry("stop");
    }
    format!(
        "{}\n",
        serde_json::to_string_pretty(&json!({ "hooks": hooks })).unwrap_or_default()
    )
}

/// Set up VS Code (Copilot) in the project (see the module docs).
pub fn install(ctx: &SetupContext) -> Result<Vec<String>, String> {
    let mcp = ctx.project(MCP);
    let mut lines = vec![merge_json_report(
        &mcp,
        &mcp_patch(&json_file(&mcp), ctx),
        ctx,
    )];
    let agents = ctx.project("AGENTS.md");
    lines.push(report(
        upsert_block(&agents, &ctx.instructions())?,
        &agents,
        ctx,
    ));
    let hooks = ctx.project(HOOKS);
    lines.push(report(
        write_owned(&hooks, &hooks_file(ctx), false)?,
        &hooks,
        ctx,
    ));
    lines.extend(install_skill(&ctx.project(".agents/skills"), ctx)?);
    Ok(lines)
}

/// The MCP entry to paste into `.vscode/mcp.json`.
pub fn mcp_config(ctx: &SetupContext) -> String {
    if ctx.remote {
        let snippet = json!({ "servers": { "post-cortex": server_entry(ctx) } });
        return format!(
            "# Merge into .vscode/mcp.json (\"servers\").\n# {REMOTE_TOKEN_NOTE}\n{}\n",
            serde_json::to_string_pretty(&snippet).unwrap_or_default()
        );
    }
    let snippet = json!({
        "servers": { "post-cortex": server_entry(ctx) },
        "inputs": [token_input()],
    });
    format!(
        "# Merge into .vscode/mcp.json (\"servers\" and \"inputs\").\n\
         # VS Code asks once for the token (input pc-auth-token); leave it empty when the daemon runs without auth.\n{}\n",
        serde_json::to_string_pretty(&snippet).unwrap_or_default()
    )
}

/// Whether `tool` changes files: VS Code's edit tools (`create_file`,
/// `replace_string_in_file`, `apply_patch`, ...) and the Claude Code names.
fn is_edit_tool(tool: &str) -> bool {
    let t = tool.to_ascii_lowercase();
    [
        "edit",
        "replace",
        "write",
        "patch",
        "createfile",
        "create_file",
    ]
    .iter()
    .any(|k| t.contains(k))
}

/// Whether `tool` reads a file: VS Code's `read_file` (`copilot_readFile`)
/// and the Claude Code name `Read` its hooks may report.
fn is_read_tool(tool: &str) -> bool {
    let t = tool.to_ascii_lowercase();
    t == "read" || t.contains("read_file") || t.contains("readfile")
}

/// The files a PreToolUse payload is about to edit (none for tools that do
/// not edit), relative to the session's `cwd` when given absolute.
fn edited_paths(input: &Value) -> Vec<String> {
    paths_of(input, is_edit_tool)
}

/// The files a PostToolUse payload read (none for tools that do not read).
fn read_paths(input: &Value) -> Vec<String> {
    paths_of(input, is_read_tool)
}

/// The files a tool call names when its tool passes `kind`, relative to the
/// session's `cwd` when given absolute, at most [`MAX_FILES`].
fn paths_of(input: &Value, kind: fn(&str) -> bool) -> Vec<String> {
    if !input["tool_name"].as_str().is_some_and(kind) {
        return Vec::new();
    }
    let args = &input["tool_input"];
    let named = |v: &Value| -> Vec<String> {
        ["filePath", "file_path", "path"]
            .iter()
            .filter_map(|key| v[*key].as_str().map(str::to_string))
            .collect()
    };
    let mut paths = named(args);
    for list in ["files", "replacements", "edits"] {
        for item in args[list].as_array().into_iter().flatten() {
            match item.as_str() {
                Some(p) => paths.push(p.to_string()),
                None => paths.extend(named(item)),
            }
        }
    }
    for key in ["input", "patch", "command"] {
        if let Some(patch) = args[key].as_str() {
            paths.extend(patch_paths(patch));
        }
    }
    let cwd = input["cwd"].as_str();
    let mut out: Vec<String> = Vec::new();
    for p in paths {
        let p = project_relative(p.trim(), cwd);
        if !p.is_empty() && !out.contains(&p) {
            out.push(p);
        }
    }
    out.truncate(MAX_FILES);
    out
}

/// The text of a transcript message: a string `content`, the text blocks
/// of a `content` array, a `text`, or the same inside `message`.
fn message_text(v: &Value) -> Option<String> {
    let text = match &v["content"] {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|b| b.as_str().or_else(|| b["text"].as_str()))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => match v["text"].as_str() {
            Some(s) => s.to_string(),
            None => return v.get("message").and_then(message_text),
        },
    };
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// Collect, in order, the assistant texts anywhere in `v`: objects with
/// `role` or `type` `assistant` (Claude-style and chat-style transcripts)
/// and VS Code chat `response` parts (`value` strings).
fn assistant_texts(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Object(map) => {
            let role = map.get("role").or_else(|| map.get("type"));
            if role
                .and_then(Value::as_str)
                .is_some_and(|r| r == "assistant" || r == "assistant.message" || r == "model")
                && let Some(text) = message_text(v)
            {
                out.push(text);
                return;
            }
            if let Some(parts) = map.get("response").and_then(Value::as_array) {
                let text: Vec<&str> = parts.iter().filter_map(|p| p["value"].as_str()).collect();
                let text = text.join("").trim().to_string();
                if !text.is_empty() {
                    out.push(text);
                }
            }
            for (key, child) in map {
                if key != "response" {
                    assistant_texts(child, out);
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|i| assistant_texts(i, out)),
        _ => {}
    }
}

/// The assistant's last reply in a transcript (JSON or JSON lines). VS
/// Code does not document the transcript format, so this is best effort:
/// `None` when nothing is recognised.
fn last_reply(transcript: &str) -> Option<String> {
    let values: Vec<Value> = match serde_json::from_str::<Value>(transcript) {
        Ok(v) => vec![v],
        Err(_) => transcript
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect(),
    };
    let mut texts = Vec::new();
    for v in &values {
        assistant_texts(v, &mut texts);
    }
    texts.pop()
}

/// The turn a capture hook stores: the prompt (UserPromptSubmit) or the
/// last reply from the transcript (Stop).
fn captured_turn(input: &Value, role: &'static str) -> Option<Turn> {
    let text = if role == "user" {
        input["prompt"].as_str()?.trim().to_string()
    } else {
        let path = input["transcript_path"].as_str()?;
        last_reply(&std::fs::read_to_string(path).ok()?)?
    };
    if text.is_empty() {
        return None;
    }
    let session = input["session_id"].as_str().map(str::to_string);
    Some(Turn {
        turn_id: turn_id(session.as_deref().unwrap_or("vscode"), role, &text),
        role,
        text,
        source_session: session,
        tools: Vec::new(),
        files: Vec::new(),
    })
}

/// Handle one VS Code hook call (see the module docs for the events).
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
        "post-read" => Ok(env
            .file_memory(
                &read_paths(&input),
                FileMoment::Read,
                input["session_id"].as_str(),
            )
            .await
            .map(|memory| file_memory_output("PostToolUse", memory))),
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
