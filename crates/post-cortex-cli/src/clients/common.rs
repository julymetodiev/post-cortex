// Copyright (c) 2026 Julius ML
// MIT License

//! What every client module shares: the setup context, file helpers that
//! merge into existing configs instead of overwriting them, the shared
//! instructions block, and the hook side (the daemon calls and the texts
//! hooks put in front of the agent).
//!
//! The hooks send their events to the backend's module host (`POST
//! /api/events`), as the Claude Code hooks do; the memory module answers
//! them.

use std::path::{Path, PathBuf};

use crate::clients::Client;

mod files;
mod hook_env;

pub use files::*;
pub use hook_env::*;

/// Everything `install` and `mcp_config` need.
pub struct SetupContext {
    /// The project directory (where `pcx setup` runs); the home directory
    /// for a global setup, so a client's project paths (`.codex/...`,
    /// `.cursor/...`) become its user-level ones.
    pub project_dir: PathBuf,
    pub session_id: String,
    pub workspace_id: String,
    /// Base URL of the daemon the client and its hooks talk to, without a
    /// trailing slash (`http://127.0.0.1:3737`).
    pub daemon_url: String,
    /// Install the capture hooks (conversation turns to the message stream).
    pub capture: bool,
    /// The backend profile the client and its hooks use (`cloud`, `self`,
    /// ...; see `profiles.rs`).
    pub profile: String,
    /// Whether the profile is remote: the client then runs `pcx mcp
    /// --profile` (stdio) instead of talking HTTP to the daemon, so no token
    /// is ever written into its config.
    pub remote: bool,
    /// A global (user-level) setup: no ids anywhere, the hooks resolve the
    /// project from the directory the client runs in (`project.rs`).
    pub global: bool,
}

/// How a client reaches post-cortex: the one decision every client module
/// takes its MCP entry from ([`SetupContext::mcp_entry`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpEntry {
    /// A self-hosted daemon, over Streamable HTTP at `url` (its `/mcp`).
    Http { url: String },
    /// A remote profile: the client starts `command args...`, the pcx stdio
    /// proxy, which resolves the URL and token from the profile.
    Stdio { command: String, args: Vec<String> },
}

impl SetupContext {
    /// The daemon's MCP endpoint.
    pub fn mcp_url(&self) -> String {
        format!("{}/mcp", self.daemon_url)
    }

    /// Path inside the project.
    pub fn project(&self, rel: &str) -> PathBuf {
        self.project_dir.join(rel)
    }

    /// The MCP entry `client` gets: stdio through `pcx mcp --profile <P>
    /// --author <client>` for a remote profile, HTTP to the daemon for a
    /// self-hosted one.
    pub fn mcp_entry(&self, client: Client) -> McpEntry {
        if self.remote {
            McpEntry::Stdio {
                command: pcx_program(),
                args: vec![
                    "mcp".into(),
                    "--profile".into(),
                    self.profile.clone(),
                    "--author".into(),
                    client.author().into(),
                ],
            }
        } else {
            McpEntry::Http {
                url: self.mcp_url(),
            }
        }
    }

    /// The `pcx hook` command line a client's hook runs for `event`, with
    /// the session, workspace (project mode only: a global hook resolves
    /// them) and profile baked in (never a token: `pcx hook` resolves it
    /// from the profile). A self-hosted daemon's URL is
    /// given too, so a `--capture-url` daemon is reached. It starts with the
    /// absolute path of `pcx` ([`pcx_program`], quoted when it has spaces):
    /// GUI clients (Cursor, VS Code, Windsurf) run hooks with a minimal
    /// PATH.
    pub fn hook_command(&self, client: Client, event: &str) -> String {
        let pcx = pcx_program();
        let pcx = if pcx.contains(char::is_whitespace) {
            format!("\"{pcx}\"")
        } else {
            pcx
        };
        let mut command = format!("{pcx} hook {} {event}", client.arg());
        if !self.global {
            command.push_str(&format!(
                " --session {} --workspace {}",
                self.session_id, self.workspace_id
            ));
        }
        command.push_str(&format!(" --profile {}", self.profile));
        if !self.remote {
            command.push_str(&format!(" --url {}", self.daemon_url));
        }
        command
    }

    /// The shared instructions block (see [`agents_md_block`]); without
    /// ids for a global setup.
    pub fn instructions(&self) -> String {
        if self.global {
            global_agents_md_block()
        } else {
            agents_md_block(&self.session_id, &self.workspace_id)
        }
    }
}

/// What `mcp_config` says about the token of a remote profile's entry.
pub const REMOTE_TOKEN_NOTE: &str =
    "pcx mcp reads the token stored by `pcx login`; the entry holds none.";

/// `ctx` switched to the `cloud` profile (tests of the remote entries).
#[cfg(test)]
pub fn remote(ctx: SetupContext) -> SetupContext {
    SetupContext {
        daemon_url: "https://api.pcx.is".into(),
        profile: "cloud".into(),
        remote: true,
        ..ctx
    }
}

/// The `args` every client's stdio entry carries for the `cloud` profile.
#[cfg(test)]
pub fn cloud_args(client: Client) -> serde_json::Value {
    serde_json::json!(["mcp", "--profile", "cloud", "--author", client.author()])
}

/// [`pcx_command`], except under test, where it is plain `pcx` so tests do
/// not depend on the machine's PATH.
pub fn pcx_program() -> String {
    if cfg!(test) {
        "pcx".to_string()
    } else {
        pcx_command()
    }
}

/// The `pcx` to run: the one on PATH (a stable symlink survives upgrades),
/// else this binary, else plain `pcx`. GUI apps start servers and hooks
/// with a minimal PATH, hence the absolute path.
pub fn pcx_command() -> String {
    let name = if cfg!(windows) { "pcx.exe" } else { "pcx" };
    std::env::var_os("PATH")
        .and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|dir| dir.join(name))
                .find(|p| p.is_file())
        })
        .or_else(|| std::env::current_exe().ok())
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "pcx".into())
}

/// The user's config directory for apps that follow XDG on every unix
/// (macOS included): `$XDG_CONFIG_HOME` when set, else `~/.config`;
/// `%APPDATA%` on Windows. `dirs::config_dir` is `~/Library/Application
/// Support` on macOS, which these apps do not read.
pub fn user_config_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        return dirs::config_dir();
    }
    match std::env::var_os("XDG_CONFIG_HOME") {
        Some(dir) if !dir.is_empty() => Some(PathBuf::from(dir)),
        _ => Some(dirs::home_dir()?.join(".config")),
    }
}

/// `path` relative to `root` when it is inside it, else as given.
pub fn project_relative(path: &str, root: Option<&str>) -> String {
    match root.and_then(|root| Path::new(path).strip_prefix(root).ok()) {
        Some(rel) => rel.display().to_string(),
        None => path.to_string(),
    }
}

/// The files an `apply_patch` patch adds or updates, in order and once
/// each: the paths of `*** Update File:`, `*** Add File:` and `*** Move
/// to:` lines (deleted files are skipped).
pub fn patch_paths(patch: &str) -> Vec<String> {
    let mut paths: Vec<String> = Vec::new();
    for line in patch.lines() {
        let line = line.trim_start();
        let path = ["*** Update File:", "*** Add File:", "*** Move to:"]
            .iter()
            .find_map(|prefix| line.strip_prefix(prefix))
            .map(str::trim);
        if let Some(path) = path
            && !path.is_empty()
            && !paths.iter().any(|p| p == path)
        {
            paths.push(path.to_string());
        }
    }
    paths
}

/// The instructions every client gets (AGENTS.md or its rules file), with
/// the session and workspace ids filled in.
pub fn agents_md_block(session_id: &str, workspace_id: &str) -> String {
    include_str!("../../templates/clients/AGENTS.md")
        .replace("<SESSION_ID>", session_id)
        .replace("<WORKSPACE_ID>", workspace_id)
}

/// [`agents_md_block`] for a user-level instructions file: no ids (the
/// session-start hook names the project's session and workspace).
pub fn global_agents_md_block() -> String {
    include_str!("../../templates/clients/AGENTS.md")
        .lines()
        .filter(|line| {
            !(line.contains("<SESSION_ID>")
                || line.contains("<WORKSPACE_ID>")
                || *line == "|---|---|")
        })
        .collect::<Vec<_>>()
        .join("\n")
        .replace(
            "This project keeps its long-term memory in post-cortex (MCP server `post-cortex`).",
            "Projects keep their long-term memory in post-cortex (MCP server `post-cortex`). \
             The session and workspace of the current project are given at session start \
             (`[PCX] Session: ... | Workspace: ...`).",
        )
        .replace("with `session_id` above", "with that session ID")
        .replace("\n\n\n", "\n\n")
        + "\n"
}

/// The skill files (same as Claude Code's), as (relative path, content).
pub fn skill_files() -> &'static [(&'static str, &'static str)] {
    crate::setup::SKILL_FILES
}

/// Install the `post-cortex-memory` skill into `dir` (a skills directory,
/// e.g. `.agents/skills`); an existing copy is left alone.
pub fn install_skill(dir: &Path, ctx: &SetupContext) -> Result<Vec<String>, String> {
    let skill = dir.join("post-cortex-memory");
    if skill.join("SKILL.md").exists() {
        return Ok(vec![report(Change::Unchanged, &skill, ctx)]);
    }
    for (rel, content) in skill_files() {
        write(&skill.join(rel), content)?;
    }
    Ok(vec![report(Change::Created, &skill, ctx)])
}

#[cfg(test)]
mod tests;
