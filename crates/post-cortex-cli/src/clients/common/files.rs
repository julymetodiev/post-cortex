// Copyright (c) 2026 Julius ML
// MIT License

//! Files a client setup writes: created or updated in place, merged into
//! existing configs (JSON, TOML, a marked block, hooks) instead of
//! overwriting them.

use std::path::Path;

use serde_json::Value;

use crate::clients::Client;
use crate::clients::common::SetupContext;

/// Where a file write left the file, for the setup report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Created,
    Updated,
    Unchanged,
}

/// `path` as the setup report shows it: relative to the project when
/// inside it, else with `~` for the home directory.
pub fn report_path(path: &Path, ctx: &SetupContext) -> String {
    let prefix = if ctx.global { "~/" } else { "" };
    path.strip_prefix(&ctx.project_dir)
        .map(|p| format!("{prefix}{}", p.display()))
        .or_else(|_| {
            dirs::home_dir()
                .and_then(|home| {
                    path.strip_prefix(home)
                        .ok()
                        .map(|p| format!("~/{}", p.display()))
                })
                .ok_or(())
        })
        .unwrap_or_else(|_| path.display().to_string())
}

/// `"  Created: <path>"` and the like (see [`report_path`]).
pub fn report(change: Change, path: &Path, ctx: &SetupContext) -> String {
    let shown = report_path(path, ctx);
    match change {
        Change::Created => format!("  Created: {shown}"),
        Change::Updated => format!("  Updated: {shown}"),
        Change::Unchanged => format!("  {shown} already set up, left as it is"),
    }
}

pub fn write(path: &Path, content: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create {}: {e}", parent.display()))?;
    }
    std::fs::write(path, content).map_err(|e| format!("Failed to write {}: {e}", path.display()))
}

fn read(path: &Path) -> Result<Option<String>, String> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("Failed to read {}: {e}", path.display())),
    }
}

/// Write a file post-cortex owns (a plugin, a hook script): created, or
/// replaced when its content differs.
pub fn write_owned(path: &Path, content: &str, executable: bool) -> Result<Change, String> {
    let change = match read(path)? {
        None => Change::Created,
        Some(existing) if existing == content => return Ok(Change::Unchanged),
        Some(_) => Change::Updated,
    };
    write(path, content)?;
    if executable {
        make_executable(path)?;
    }
    Ok(change)
}

/// Let the owner run `path` (mode 0755).
#[cfg(unix)]
fn make_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .map_err(|e| format!("Failed to chmod {}: {e}", path.display()))
}

/// Nothing to do: a file is run by its extension's handler.
#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<(), String> {
    Ok(())
}

/// Begin and end markers of the block post-cortex keeps in a markdown file
/// it shares with the user (AGENTS.md, a rules file).
pub const BLOCK_BEGIN: &str = "<!-- post-cortex:begin -->";
pub const BLOCK_END: &str = "<!-- post-cortex:end -->";

/// Put `block` between the markers in a markdown file: replaced in place
/// when the markers exist, else appended (after a blank line); the rest of
/// the file is kept. A new file is just the block.
pub fn upsert_block(path: &Path, block: &str) -> Result<Change, String> {
    let wrapped = format!("{BLOCK_BEGIN}\n{}\n{BLOCK_END}\n", block.trim_end());
    let Some(existing) = read(path)? else {
        write(path, &wrapped)?;
        return Ok(Change::Created);
    };
    let updated = match (existing.find(BLOCK_BEGIN), existing.find(BLOCK_END)) {
        (Some(start), Some(end)) if start < end => {
            let after = &existing[end + BLOCK_END.len()..];
            let after = after.strip_prefix('\n').unwrap_or(after);
            format!("{}{wrapped}{after}", &existing[..start])
        }
        _ if existing.trim().is_empty() => wrapped,
        _ => format!("{}\n\n{wrapped}", existing.trim_end()),
    };
    if updated == existing {
        return Ok(Change::Unchanged);
    }
    write(path, &updated)?;
    Ok(Change::Updated)
}

/// Deep-merge `patch` into a JSON file: objects merge key by key, a `null`
/// in `patch` removes the key (as in JSON Merge Patch), anything else
/// replaces the existing value; keys the patch does not name are kept. A missing file becomes `patch`. Fails (writing nothing)
/// when the file is not a JSON object, so a hand-edited file is never lost.
/// JSON with comments (JSONC) is not parsed and fails the same way.
pub fn merge_json(path: &Path, patch: &Value) -> Result<Change, String> {
    let existing = read(path)?;
    let mut value = match &existing {
        None => Value::Object(Default::default()),
        Some(text) if text.trim().is_empty() => Value::Object(Default::default()),
        Some(text) => serde_json::from_str::<Value>(text).map_err(|e| {
            format!(
                "{} is not plain JSON ({e}); add the post-cortex entry by hand (pcx mcp-config prints it)",
                path.display()
            )
        })?,
    };
    if !value.is_object() {
        return Err(format!("{} is not a JSON object", path.display()));
    }
    merge_value(&mut value, patch);
    let text = format!(
        "{}\n",
        serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?
    );
    match existing {
        Some(old) if old == text => Ok(Change::Unchanged),
        Some(old) if serde_json::from_str::<Value>(&old).ok().as_ref() == Some(&value) => {
            Ok(Change::Unchanged)
        }
        Some(_) => {
            write(path, &text)?;
            Ok(Change::Updated)
        }
        None => {
            write(path, &text)?;
            Ok(Change::Created)
        }
    }
}

/// [`merge_json`] as a report line: the change, or `"  Skipped: <why>"`
/// when the file is refused, so one refused file does not stop the rest
/// of the setup.
pub fn merge_json_report(path: &Path, patch: &Value, ctx: &SetupContext) -> String {
    match merge_json(path, patch) {
        Ok(change) => report(change, path, ctx),
        Err(e) => skipped(&e),
    }
}

/// The report line for a file setup left alone, with the reason.
pub fn skipped(reason: &str) -> String {
    format!("  Skipped: {reason}")
}

/// The JSON in `path`; `Null` when the file is missing or not JSON
/// (`merge_json` then reports the parse error itself).
pub fn json_file(path: &Path) -> Value {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(Value::Null)
}

/// Whether a hook entry runs `pcx hook <client> ...`: the program before
/// ` hook <client> ` is `pcx`, by any path (setup writes the absolute one;
/// older setups wrote plain `pcx`).
pub fn is_our_hook(entry: &Value, client: Client) -> bool {
    let marker = format!(" hook {} ", client.arg());
    entry["command"].as_str().is_some_and(|command| {
        command.split_once(&marker).is_some_and(|(program, _)| {
            Path::new(program.trim().trim_matches('"'))
                .file_stem()
                .is_some_and(|stem| stem == "pcx")
        })
    })
}

/// The per-event hook arrays to merge into a hooks object (`existing`, the
/// object holding the event keys, may be `Null`). `merge_json` replaces
/// arrays, so each array is built whole here: the entries already there
/// minus post-cortex's, then `ours`. An entry with its own `hooks` list (a
/// Claude-style group: Codex, Gemini CLI) keeps the person's hooks of that
/// list and goes once none is left; a flat entry (Cursor, Windsurf) goes
/// when it is ours. `events` names every event post-cortex may write, with
/// an empty `ours` where nothing is wanted now, so a hook turned off (no
/// more capture) is removed. An event that was absent and gets nothing
/// stays absent.
pub fn hook_arrays(existing: &Value, client: Client, events: Vec<(&str, Vec<Value>)>) -> Value {
    let mut out = serde_json::Map::new();
    for (event, ours) in events {
        let current = existing[event].as_array();
        let mut entries: Vec<Value> = current
            .into_iter()
            .flatten()
            .filter_map(|entry| match entry["hooks"].as_array() {
                Some(hooks) => {
                    let kept: Vec<Value> = hooks
                        .iter()
                        .filter(|h| !is_our_hook(h, client))
                        .cloned()
                        .collect();
                    if kept.is_empty() {
                        return None;
                    }
                    let mut group = entry.clone();
                    group["hooks"] = Value::Array(kept);
                    Some(group)
                }
                None => (!is_our_hook(entry, client)).then(|| entry.clone()),
            })
            .collect();
        entries.extend(ours);
        if current.is_some() || !entries.is_empty() {
            out.insert(event.to_string(), Value::Array(entries));
        }
    }
    Value::Object(out)
}

fn merge_value(target: &mut Value, patch: &Value) {
    match (target, patch) {
        (Value::Object(t), Value::Object(p)) => {
            for (key, value) in p {
                if value.is_null() {
                    t.remove(key);
                    continue;
                }
                match t.get_mut(key) {
                    Some(existing) if existing.is_object() && value.is_object() => {
                        merge_value(existing, value)
                    }
                    _ if value.is_object() => {
                        // Merged into an empty object, so its nulls go.
                        let mut fresh = Value::Object(Default::default());
                        merge_value(&mut fresh, value);
                        t.insert(key.clone(), fresh);
                    }
                    _ => {
                        t.insert(key.clone(), value.clone());
                    }
                }
            }
        }
        (target, patch) => *target = patch.clone(),
    }
}

/// Put a TOML section into a file: the section under `header` (e.g.
/// `[mcp_servers.post-cortex]`) and its subtables (`[mcp_servers.post-cortex.env]`)
/// are replaced in place when there, else `section` is appended. Every
/// other line is left byte for byte (comments kept). `section` is the whole
/// section text, header included.
pub fn upsert_toml_section(path: &Path, header: &str, section: &str) -> Result<Change, String> {
    let existing = read(path)?;
    let section = section.trim_end();
    let text = match &existing {
        Some(text) if !text.trim().is_empty() => {
            let lines: Vec<&str> = text.lines().collect();
            match lines.iter().position(|l| l.trim() == header) {
                Some(start) => {
                    let sub = format!("{}.", header.trim_end_matches(']'));
                    let end = lines[start + 1..]
                        .iter()
                        .position(|l| {
                            let l = l.trim_start();
                            l.starts_with('[') && !l.starts_with(&sub)
                        })
                        .map_or(lines.len(), |i| start + 1 + i);
                    // Comments right above the next section belong to it.
                    let mut end = end;
                    if end < lines.len() {
                        while end > start + 1 && {
                            let l = lines[end - 1].trim();
                            l.is_empty() || l.starts_with('#')
                        } {
                            end -= 1;
                        }
                        while lines[end].trim().is_empty() {
                            end += 1;
                        }
                    }
                    let mut out: Vec<String> =
                        lines[..start].iter().map(|l| l.to_string()).collect();
                    out.push(section.to_string());
                    if end < lines.len() {
                        out.push(String::new());
                        out.extend(lines[end..].iter().map(|l| l.to_string()));
                    }
                    format!("{}\n", out.join("\n").trim_end())
                }
                None => format!("{}\n\n{section}\n", text.trim_end()),
            }
        }
        _ => format!("{section}\n"),
    };
    match &existing {
        Some(old) if *old == text => Ok(Change::Unchanged),
        Some(_) => write(path, &text).map(|_| Change::Updated),
        None => write(path, &text).map(|_| Change::Created),
    }
}

/// The keys an MCP server entry uses for its transport, across clients.
const TRANSPORT_KEYS: [&str; 9] = [
    "type",
    "url",
    "serverUrl",
    "httpUrl",
    "headers",
    "command",
    "args",
    "env",
    "transport",
];

/// `entry` as a [`merge_json`] patch that switches transports cleanly: every
/// transport key `entry` lacks is `null`, so an HTTP entry left by an
/// earlier setup loses its `url` and `headers` when it becomes stdio (and
/// the other way round), while the person's own keys (`disabled`,
/// `autoApprove`, ...) stay.
pub fn transport_patch(entry: Value) -> Value {
    let mut entry = entry;
    if let Some(map) = entry.as_object_mut() {
        for key in TRANSPORT_KEYS {
            map.entry(key).or_insert(Value::Null);
        }
    }
    entry
}
