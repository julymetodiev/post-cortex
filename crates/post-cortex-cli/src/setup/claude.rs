// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! Claude Code: the post-cortex plugin carries the MCP server, the skill and
//! the hooks (owner 2026-11-06: post-cortex is used only through the
//! plugin), and finds a project by its pin, `.post-cortex.json`. So setup
//! writes nothing of Claude Code's: it says how to install the plugin, sets
//! the plugin's capture switch, and takes out what setups before the plugin
//! wrote - the PCX section of CLAUDE.md, post-cortex's hooks in
//! `.claude/settings.json` and their scripts in `.claude/hooks/`, the
//! post-cortex-agents sub-agents and the user-level copy of the skill.

use std::path::Path;

use crate::profiles::{self, KEYS_URL, Profile, Store};

/// The hook scripts setups before the plugin put in `.claude/hooks/`.
const OLD_HOOK_SCRIPTS: &[&str] = &[
    "pcx-banner.py",
    "pcx-capture.py",
    "pcx-card.py",
    "pcx-compact-reinject.py",
    "pcx-compact-reinject.sh",
    "pcx-file-context.py",
    "pcx-stop-check.py",
    "pcx_resolve.py",
];

/// The headings of the PCX section setups before the plugin put in
/// CLAUDE.md, after its `## Session`.
const SECTION_HEADINGS: &[&str] = &[
    "## Rules",
    "### Interaction Types",
    "## MCP Tools",
    "## Hooks",
];

/// The skill files bundled into `pcx` for the clients without a plugin. The
/// canonical copy lives in `skills/post-cortex-memory/` at the repository
/// root; `templates/skills/` is kept identical (checked by
/// `bundled_skill_matches_repository_copy`).
pub const SKILL_FILES: &[(&str, &str)] = &[
    (
        "SKILL.md",
        include_str!("../../templates/skills/post-cortex-memory/SKILL.md"),
    ),
    (
        "references/tools.md",
        include_str!("../../templates/skills/post-cortex-memory/references/tools.md"),
    ),
    (
        "references/setup.md",
        include_str!("../../templates/skills/post-cortex-memory/references/setup.md"),
    ),
];

/// Claude Code for `backend`: the plugin to install, its capture switch
/// set (`hosts.claude-code.capture` in `config.json`, read by its hooks),
/// and what setups before the plugin left in the user's `~/.claude/` taken
/// out. The lines to print.
pub fn setup_plugin(
    store: &Store,
    backend: &Profile,
    capture: bool,
) -> Result<Vec<String>, String> {
    let mut lines = claude_plugin_lines(backend);
    let author = crate::clients::Client::ClaudeCode.author();
    if store.host(author)?.capture != capture {
        store.set_host(author, "capture", serde_json::Value::Bool(capture))?;
    }
    lines.push(format!(
        "  capture {} for the plugin (hosts.{author}.capture in {}/config.json)",
        if capture { "on" } else { "off" },
        store.dir().display()
    ));
    let home = dirs::home_dir().ok_or_else(|| "Cannot determine home directory".to_string())?;
    for old in [
        ".claude/agents/post-cortex-agents",
        ".claude/skills/post-cortex-memory",
    ] {
        let dir = home.join(old);
        if dir.exists() {
            std::fs::remove_dir_all(&dir)
                .map_err(|e| format!("Failed to remove {}: {e}", dir.display()))?;
            lines.push(format!("  Removed: ~/{old} (the plugin carries it)"));
        }
    }
    Ok(lines)
}

/// What a Claude Code setup prints: the plugin carries the MCP server, the
/// skill and the hooks, so there is nothing to write.
fn claude_plugin_lines(backend: &Profile) -> Vec<String> {
    let mut lines = vec![
        "  Install the post-cortex plugin (MCP server, skill and hooks) in Claude Code:".into(),
        "    /plugin marketplace add julymetodiev/post-cortex".into(),
        "    /plugin install post-cortex@post-cortex".into(),
    ];
    if backend.url == profiles::CLOUD_URL {
        lines.push(format!(
            "  When asked: keep the url, api_key = your key ({KEYS_URL})"
        ));
    } else {
        lines.push(format!(
            "  When asked: url = {}, api_key = {}",
            backend.url,
            if backend.remote() {
                "the backend's API key"
            } else {
                "the daemon's auth_token (empty without one)"
            }
        ));
    }
    lines
}

/// Take out of project `cwd` what setups before the plugin wrote there: the
/// PCX section of CLAUDE.md (the file too when nothing else is left in it),
/// post-cortex's hooks in `.claude/settings.json` (the person's stay) and
/// their scripts. The lines to print.
pub fn retire_project(cwd: &Path) -> Result<Vec<String>, String> {
    let mut lines = Vec::new();
    let claude_md = cwd.join("CLAUDE.md");
    if claude_md.exists() {
        let text = std::fs::read_to_string(&claude_md).map_err(|e| format!("Read error: {e}"))?;
        if let Some(rest) = without_pcx_section(&text) {
            if rest
                .lines()
                .all(|l| l.trim().is_empty() || l.trim() == "# CLAUDE.md")
            {
                std::fs::remove_file(&claude_md)
                    .map_err(|e| format!("Failed to remove CLAUDE.md: {e}"))?;
                lines.push("  Removed: CLAUDE.md (it held only the PCX section)".into());
            } else {
                std::fs::write(&claude_md, rest)
                    .map_err(|e| format!("Failed to write CLAUDE.md: {e}"))?;
                lines.push("  Updated: CLAUDE.md (the PCX section taken out)".into());
            }
        }
    }
    let settings_path = cwd.join(".claude/settings.json");
    if settings_path.exists() {
        let text =
            std::fs::read_to_string(&settings_path).map_err(|e| format!("Read error: {e}"))?;
        match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(mut settings) => {
                if remove_pcx_hooks(&mut settings)? {
                    let json = serde_json::to_string_pretty(&settings)
                        .map_err(|e| format!("JSON error: {e}"))?;
                    std::fs::write(&settings_path, format!("{json}\n"))
                        .map_err(|e| format!("Write error: {e}"))?;
                    lines.push(
                        "  Updated: .claude/settings.json (post-cortex's hooks taken out)".into(),
                    );
                }
            }
            Err(_) => lines.push("  .claude/settings.json is not valid JSON, left as it is".into()),
        }
    }
    let hooks_dir = cwd.join(".claude/hooks");
    for name in OLD_HOOK_SCRIPTS {
        let path = hooks_dir.join(name);
        if path.exists() {
            std::fs::remove_file(&path)
                .map_err(|e| format!("Failed to remove {}: {e}", path.display()))?;
            lines.push(format!("  Removed: .claude/hooks/{name}"));
        }
    }
    Ok(lines)
}

/// `text`, a CLAUDE.md, without the PCX section setups before the plugin
/// put in it: from its `## Session` heading (with the session id) through
/// its Rules, Interaction Types, MCP Tools and Hooks, up to the next other
/// heading, or the `---` setup put after it; `None` when it has none.
fn without_pcx_section(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.iter().enumerate().position(|(i, line)| {
        line.trim() == "## Session"
            && lines[i + 1..]
                .iter()
                .take(4)
                .any(|l| l.contains("Session ID"))
    })?;
    let end = lines[start + 1..]
        .iter()
        .position(|line| {
            let line = line.trim();
            line == "---"
                || (line.starts_with('#')
                    && line.trim_start_matches('#').starts_with(' ')
                    && !SECTION_HEADINGS.contains(&line))
        })
        .map_or(lines.len(), |at| start + 1 + at);
    // The `---` setup put between the section and the file's own text goes
    // with it.
    let after = if lines.get(end).is_some_and(|l| l.trim() == "---") {
        end + 1
    } else {
        end
    };
    let mut kept: Vec<&str> = lines[..start].to_vec();
    while kept.last().is_some_and(|l| l.trim().is_empty()) {
        kept.pop();
    }
    let rest: Vec<&str> = lines[after..]
        .iter()
        .copied()
        .skip_while(|l| l.trim().is_empty())
        .collect();
    if !kept.is_empty() && !rest.is_empty() {
        kept.push("");
    }
    kept.extend(rest);
    Some(kept.join("\n") + "\n")
}

/// What identifies a PCX hook command across setups: its script name
/// without the extension (`pcx-stop-check`, `pcx-compact-reinject` for the
/// `.sh` of older setups and the `.py` after), or the `[PCX]` banner of the
/// prompt hook. The ids or backend in it may differ.
fn hook_key(command: &str) -> String {
    if let Some(start) = command.find("pcx-") {
        let script: String = command[start..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_'))
            .collect();
        if let Some(stem) = script
            .strip_suffix(".py")
            .or_else(|| script.strip_suffix(".sh"))
        {
            return stem.to_string();
        }
    }
    if command.contains("[PCX]") {
        return "[PCX]".to_string();
    }
    command.to_string()
}

/// The [`hook_key`]s of every PCX hook setups before the plugin wrote: the
/// scripts of [`OLD_HOOK_SCRIPTS`] and the prompt banner.
fn managed_hook_keys() -> Vec<String> {
    OLD_HOOK_SCRIPTS
        .iter()
        .map(|name| hook_key(name))
        .chain(["[PCX]".to_string()])
        .collect()
}

/// Take post-cortex's hooks out of `settings.hooks`: every hook whose
/// command is one of [`managed_hook_keys`] goes, a group left without hooks
/// goes too, an event left empty as well, and `hooks` when nothing is left
/// in it. The person's own hooks and every other key stay. Whether anything
/// changed.
fn remove_pcx_hooks(settings: &mut serde_json::Value) -> Result<bool, String> {
    let Some(settings) = settings.as_object_mut() else {
        return Err(".claude/settings.json is not a JSON object".to_string());
    };
    let Some(hooks) = settings.get_mut("hooks") else {
        return Ok(false);
    };
    let hooks = hooks
        .as_object_mut()
        .ok_or_else(|| "\"hooks\" in .claude/settings.json is not an object".to_string())?;
    let before = hooks.clone();
    let managed = managed_hook_keys();
    let ours = |hook: &serde_json::Value| {
        hook["command"]
            .as_str()
            .is_some_and(|c| managed.contains(&hook_key(c)))
    };
    let events: Vec<String> = hooks.keys().cloned().collect();
    for event in events {
        let Some(groups) = hooks[&event].as_array() else {
            continue;
        };
        let kept: Vec<serde_json::Value> = groups
            .iter()
            .filter_map(|group| match group["hooks"].as_array() {
                Some(list) => {
                    let list: Vec<serde_json::Value> =
                        list.iter().filter(|h| !ours(h)).cloned().collect();
                    if list.is_empty() {
                        return None;
                    }
                    let mut group = group.clone();
                    group["hooks"] = serde_json::Value::Array(list);
                    Some(group)
                }
                None => Some(group.clone()),
            })
            .collect();
        if kept.is_empty() {
            hooks.remove(&event);
        } else {
            hooks.insert(event, serde_json::Value::Array(kept));
        }
    }
    let changed = *hooks != before;
    if hooks.is_empty() {
        settings.remove("hooks");
    }
    Ok(changed)
}

#[cfg(test)]
mod tests;
