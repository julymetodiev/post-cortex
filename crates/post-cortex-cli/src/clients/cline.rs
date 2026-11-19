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
//!   rules reminder, `PostToolUse` the file memory (up to three files, once a
//!   task) of what `read_files` / `read_file` read, and `PreToolUse` that of
//!   a file not told yet before `write_to_file`, `replace_in_file`, `editor`
//!   and `apply_patch`. Cline shows hook context to the model on the next
//!   request, and a hook cannot refuse one call (`cancel` ends the task), so
//!   an edit of a file the agent did not read gets its memory after it is
//!   written. With `--capture`, `UserPromptSubmit` also stores the prompt and
//!   `PostToolUse` the answer of `attempt_completion` / `submit_and_exit`. Cline runs hooks only on macOS and Linux, once
//!   "Enable Hooks" is on in its settings. A script with the same name that
//!   post-cortex did not write is left alone.

use std::path::Path;

use serde_json::{Value, json};

use crate::clients::Client;
use crate::clients::common::{
    Change, FileMemory, FileMoment, HookEnv, McpEntry, REMOTE_TOKEN_NOTE, SetupContext, Turn,
    install_skill, patch_paths, project_relative, report, report_path, skipped, turn_id,
    upsert_block, write_owned,
};

/// The line marking a hook script as post-cortex's own.
const OWNED_MARK: &str = "# post-cortex hook, written by `pcx setup --client cline`";

/// `UserPromptSubmit` when setup ran with `--capture`.
const PROMPT_CAPTURE: &str = "UserPromptSubmit-capture";

/// Tools that change a file, whose path gets the file memory first.
const EDIT_TOOLS: [&str; 4] = ["write_to_file", "replace_in_file", "editor", "apply_patch"];

/// Tools that read files, whose paths get the file memory with the read.
const READ_TOOLS: [&str; 2] = ["read_files", "read_file"];

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
    let hooks = [
        ("TaskStart", "TaskStart"),
        ("TaskResume", "TaskResume"),
        ("UserPromptSubmit", prompt_event),
        ("PreToolUse", "PreToolUse"),
        ("PostToolUse", "PostToolUse"),
    ];
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
        // Cline cannot refuse one tool call (`cancel` ends the task), so a
        // held edit's memory comes as context, with the next request.
        "PreToolUse" => env
            .file_memory(&edited_files(&input), FileMoment::Edit, task(&input))
            .await
            .map(FileMemory::into_text),
        "PostToolUse" => {
            if let Some(turn) = answer_turn(&input) {
                // Stored only with capture on.
                env.record(vec![turn]).await;
            }
            env.file_memory(&read_files(&input), FileMoment::Read, task(&input))
                .await
                .map(FileMemory::into_text)
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

/// The files a `PostToolUse` of a read tool read: `read_files`'s `files`
/// (each a `{path}`, the list itself possibly a JSON string) or
/// `read_file`'s `path`, relative to the workspace root; at most
/// [`MAX_FILES`]. Empty for other tools.
fn read_files(input: &Value) -> Vec<String> {
    let tool = &input["postToolUse"];
    if !tool["toolName"]
        .as_str()
        .is_some_and(|t| READ_TOOLS.contains(&t))
    {
        return Vec::new();
    }
    let params = &tool["parameters"];
    let files = match &params["files"] {
        Value::String(text) => match serde_json::from_str(text) {
            Ok(files) => files,
            Err(e) => {
                eprintln!("pcx hook cline: read_files `files` is not JSON: {e}");
                Value::Null
            }
        },
        other => other.clone(),
    };
    let mut paths: Vec<String> = files
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| f["path"].as_str().or_else(|| f.as_str()))
        .map(str::to_string)
        .collect();
    if let Some(path) = params["path"].as_str() {
        paths.push(path.to_string());
    }
    let root = input["workspaceRoots"][0].as_str();
    paths
        .iter()
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .map(|p| project_relative(p, root))
        .take(MAX_FILES)
        .collect()
}

/// The task, the session a file's memory is told once in.
fn task(input: &Value) -> Option<&str> {
    input["taskId"].as_str().filter(|t| !t.is_empty())
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
mod tests;
