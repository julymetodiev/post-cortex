// Copyright (c) 2026 Julius ML
// MIT License

//! post-cortex for coding agents other than Claude Code.
//!
//! `pcx setup --client <client>` writes, per client, its MCP server entry,
//! the shared instructions block (AGENTS.md or the client's rules file) and,
//! where the client runs hooks or plugins, those too. The hooks call
//! `pcx hook <client> <event>`, which reads the client's hook payload on
//! stdin and prints the client's reply: the cards on session start, what the
//! memory knows about a file before it is edited, captured turns (with
//! `--capture`). `pcx mcp-config <client>` prints the MCP entry only.
//!
//! Each client lives in its own module with the same three functions:
//! `install`, `mcp_config` and `hook` (see [`common`] for the helpers).

use clap::ValueEnum;

pub mod common;

mod claude_desktop;
mod cline;
mod codex;
mod cursor;
mod gemini;
mod goose;
mod opencode;
mod roo;
mod vscode;
mod windsurf;
mod zed;

pub use common::{HookEnv, McpEntry, ProjectSource, SetupContext};

/// A coding agent `pcx setup --client` can configure.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, ValueEnum)]
pub enum Client {
    /// Claude Code (CLAUDE.md, .claude/settings.json hooks, skill): the
    /// default.
    ClaudeCode,
    /// OpenAI Codex CLI.
    Codex,
    /// Cursor (editor and CLI).
    Cursor,
    /// OpenCode.
    Opencode,
    /// Gemini CLI.
    Gemini,
    /// VS Code with GitHub Copilot agent mode.
    Vscode,
    /// Windsurf (Devin Desktop).
    Windsurf,
    /// Zed.
    Zed,
    /// Cline.
    Cline,
    /// Roo Code.
    Roo,
    /// Goose.
    Goose,
    /// Claude Desktop.
    ClaudeDesktop,
}

impl Client {
    /// The author name the client's writes carry (`X-PCX-Author`).
    pub fn author(self) -> &'static str {
        match self {
            Client::ClaudeCode => "claude-code",
            Client::Codex => "codex",
            Client::Cursor => "cursor",
            Client::Opencode => "opencode",
            Client::Gemini => "gemini-cli",
            Client::Vscode => "copilot",
            Client::Windsurf => "windsurf",
            Client::Zed => "zed",
            Client::Cline => "cline",
            Client::Roo => "roo-code",
            Client::Goose => "goose",
            Client::ClaudeDesktop => "claude-desktop",
        }
    }

    /// Name as shown to people.
    pub fn label(self) -> &'static str {
        match self {
            Client::ClaudeCode => "Claude Code",
            Client::Codex => "Codex CLI",
            Client::Cursor => "Cursor",
            Client::Opencode => "OpenCode",
            Client::Gemini => "Gemini CLI",
            Client::Vscode => "VS Code (Copilot)",
            Client::Windsurf => "Windsurf",
            Client::Zed => "Zed",
            Client::Cline => "Cline",
            Client::Roo => "Roo Code",
            Client::Goose => "Goose",
            Client::ClaudeDesktop => "Claude Desktop",
        }
    }

    /// Whether the client can be set up once for every project (`pcx setup`
    /// without `--project`): it reads a user-level MCP config and, where it
    /// runs hooks, user-level hooks, so no file in the project names the
    /// session. Claude Code gets the plugin; Claude Desktop has only a
    /// user-level config anyway. The others read their MCP entry, hooks or
    /// instructions from the project only and keep project mode.
    pub fn global(self) -> bool {
        matches!(
            self,
            Client::ClaudeCode
                | Client::Codex
                | Client::Cursor
                | Client::Gemini
                | Client::ClaudeDesktop
        )
    }

    /// The project files where `pcx setup --project` puts this client's
    /// hooks. A global hook steps aside when the project has its own there
    /// (project mode wins, so a hook never runs twice).
    pub fn project_hook_files(self) -> &'static [&'static str] {
        match self {
            Client::Codex => &[".codex/hooks.json"],
            Client::Cursor => &[".cursor/hooks.json"],
            Client::Gemini => &[".gemini/settings.json"],
            _ => &[],
        }
    }

    /// The value `--client` takes (`claude-code`, `codex`, ...).
    pub fn arg(self) -> String {
        self.to_possible_value()
            .map(|v| v.get_name().to_string())
            .unwrap_or_default()
    }
}

/// Write the client's files into the project (and, where the client only
/// reads a user-level file, into the user's config). Returns one line per
/// file created, updated or left as it was, for the setup report.
/// Claude Code is set up by `setup.rs` itself and is not handled here.
pub fn install(client: Client, ctx: &SetupContext) -> Result<Vec<String>, String> {
    match client {
        Client::ClaudeCode => Ok(Vec::new()),
        Client::Codex => codex::install(ctx),
        Client::Cursor => cursor::install(ctx),
        Client::Opencode => opencode::install(ctx),
        Client::Gemini => gemini::install(ctx),
        Client::Vscode => vscode::install(ctx),
        Client::Windsurf => windsurf::install(ctx),
        Client::Zed => zed::install(ctx),
        Client::Cline => cline::install(ctx),
        Client::Roo => roo::install(ctx),
        Client::Goose => goose::install(ctx),
        Client::ClaudeDesktop => claude_desktop::install(ctx),
    }
}

/// The client's MCP server entry, as the snippet to paste into its config
/// (`pcx mcp-config <client>`), with a line saying where it goes.
pub fn mcp_config(client: Client, ctx: &SetupContext) -> String {
    match client {
        Client::ClaudeCode => match ctx.mcp_entry(Client::ClaudeCode) {
            McpEntry::Stdio { command, args } => format!(
                "# Run once ({}):\nclaude mcp add --scope user post-cortex -- {} {}\n",
                common::REMOTE_TOKEN_NOTE,
                shell_word(&command),
                args.join(" ")
            ),
            McpEntry::Http { url } => format!(
                "# Run once:\nclaude mcp add --scope user --transport http post-cortex {url} \\\n  --header \"X-PCX-Author: claude-code\"\n"
            ),
        },
        Client::Codex => codex::mcp_config(ctx),
        Client::Cursor => cursor::mcp_config(ctx),
        Client::Opencode => opencode::mcp_config(ctx),
        Client::Gemini => gemini::mcp_config(ctx),
        Client::Vscode => vscode::mcp_config(ctx),
        Client::Windsurf => windsurf::mcp_config(ctx),
        Client::Zed => zed::mcp_config(ctx),
        Client::Cline => cline::mcp_config(ctx),
        Client::Roo => roo::mcp_config(ctx),
        Client::Goose => goose::mcp_config(ctx),
        Client::ClaudeDesktop => claude_desktop::mcp_config(ctx),
    }
}

/// Whether the project at `root` runs its own (project mode) pcx hooks for
/// `client`: one of [`Client::project_hook_files`] holds a `pcx hook
/// <client>` command with `--session`.
pub fn has_project_hooks(client: Client, root: &std::path::Path) -> bool {
    let marker = format!(" hook {} ", client.arg());
    client.project_hook_files().iter().any(|rel| {
        std::fs::read_to_string(root.join(rel)).is_ok_and(|text| {
            text.lines()
                .any(|line| line.contains(&marker) && line.contains("--session"))
        })
    })
}

/// `word` quoted for a POSIX shell when it holds anything but plain path
/// characters (a `pcx` path with spaces).
fn shell_word(word: &str) -> String {
    if word
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "-._/~:+".contains(c))
    {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}

/// Handle one hook call: `input` is the client's hook payload (stdin JSON).
/// Returns what to print on stdout (`None`: nothing). Hooks fail open: the
/// caller prints errors to stderr and exits 0, so memory never blocks the
/// agent.
pub async fn hook(
    client: Client,
    event: &str,
    input: serde_json::Value,
    env: &HookEnv,
) -> Result<Option<String>, String> {
    match client {
        Client::Codex => codex::hook(event, input, env).await,
        Client::Cursor => cursor::hook(event, input, env).await,
        Client::Gemini => gemini::hook(event, input, env).await,
        Client::Vscode => vscode::hook(event, input, env).await,
        Client::Windsurf => windsurf::hook(event, input, env).await,
        Client::Cline => cline::hook(event, input, env).await,
        Client::Goose => goose::hook(event, input, env).await,
        other => Err(format!("{} has no pcx hooks", other.label())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(dir: &std::path::Path) -> SetupContext {
        SetupContext {
            project_dir: dir.to_path_buf(),
            session_id: "S1".into(),
            workspace_id: "W1".into(),
            daemon_url: "http://127.0.0.1:3737".into(),
            capture: false,
            profile: "self".into(),
            remote: false,
            global: false,
        }
    }

    #[test]
    fn claude_code_hint_follows_the_profile() {
        let dir = tempfile::tempdir().unwrap();
        let local = mcp_config(Client::ClaudeCode, &ctx(dir.path()));
        assert!(
            local.contains("--transport http post-cortex http://127.0.0.1:3737/mcp"),
            "{local}"
        );
        let cloud = mcp_config(Client::ClaudeCode, &common::remote(ctx(dir.path())));
        assert!(
            cloud.contains(
                "claude mcp add --scope user post-cortex -- pcx mcp --profile cloud --author claude-code\n"
            ),
            "{cloud}"
        );
        assert_eq!(shell_word("/a b/pcx"), "'/a b/pcx'");
    }

    /// A project set up with `--project` keeps its own hooks; the global
    /// ones step aside there.
    #[test]
    fn project_hooks_are_detected() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!has_project_hooks(Client::Codex, dir.path()));
        let project = ctx(dir.path());
        install(Client::Codex, &project).unwrap();
        assert!(has_project_hooks(Client::Codex, dir.path()));
        assert!(!has_project_hooks(Client::Cursor, dir.path()));
        let home = tempfile::tempdir().unwrap();
        let global = SetupContext {
            project_dir: home.path().to_path_buf(),
            global: true,
            ..ctx(home.path())
        };
        install(Client::Codex, &global).unwrap();
        assert!(!has_project_hooks(Client::Codex, home.path()));
        assert!(Client::Gemini.global() && !Client::Zed.global());
    }
}
