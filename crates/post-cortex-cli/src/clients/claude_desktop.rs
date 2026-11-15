// Copyright (c) 2026 Julius ML
// MIT License

//! Claude Desktop: the MCP entry in its user-level config
//! (`claude_desktop_config.json` in the OS config directory: macOS
//! `~/Library/Application Support/Claude/`, Windows `%APPDATA%\Claude\`,
//! Linux `~/.config/Claude/`). Claude Desktop has no project files, no
//! instructions file and no hooks.
//!
//! Its config file only runs local stdio servers, so the entry starts
//! `pcx` itself in its stdio mode (no subcommand): the stdio proxy to the
//! daemon, which starts the daemon when it is not running. `PC_HOST` /
//! `PC_PORT` point it at the daemon, `PC_AUTHOR` names the writes, and the
//! proxy takes the bearer token from `daemon.toml`. A daemon that is not on
//! this machine is reached through the `mcp-remote` bridge instead. For a
//! remote profile (the hosted cloud) the entry runs `pcx mcp --profile <P>`,
//! which reads the token stored by `pcx login`.

use std::path::PathBuf;

use serde_json::{Value, json};

use crate::clients::Client;
use crate::clients::common::{
    Change, McpEntry, REMOTE_TOKEN_NOTE, SetupContext, merge_json, pcx_command, report,
    transport_patch,
};

/// The config file inside the OS config directory.
fn config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("Claude").join("claude_desktop_config.json"))
}

/// Host and port of a daemon on this machine (`http://127.0.0.1:3737`);
/// `None` for anything the stdio proxy cannot reach (https, other hosts).
fn local_host_port(daemon_url: &str) -> Option<(String, u16)> {
    let rest = daemon_url.strip_prefix("http://")?.trim_end_matches('/');
    let (host, port) = rest.rsplit_once(':')?;
    let port = port.parse().ok()?;
    matches!(host, "127.0.0.1" | "localhost").then(|| (host.to_string(), port))
}

/// The `mcpServers` entry, `pcx` being the command to run for a local
/// daemon or a remote profile.
fn server_entry(ctx: &SetupContext, pcx: &str) -> Value {
    let author = Client::ClaudeDesktop.author();
    if let McpEntry::Stdio { args, .. } = ctx.mcp_entry(Client::ClaudeDesktop) {
        return json!({ "command": pcx, "args": args });
    }
    match local_host_port(&ctx.daemon_url) {
        Some((host, port)) => json!({
            "command": pcx,
            "args": [],
            "env": {
                "PC_HOST": host,
                "PC_PORT": port.to_string(),
                "PC_AUTHOR": author,
            },
        }),
        None => {
            let mut args = vec![
                "-y".to_string(),
                "mcp-remote".into(),
                ctx.mcp_url(),
                "--header".into(),
                format!("X-PCX-Author: {author}"),
            ];
            if ctx.daemon_url.starts_with("http://") {
                args.push("--allow-http".into());
            }
            json!({ "command": "npx", "args": args })
        }
    }
}

fn patch(ctx: &SetupContext, pcx: &str) -> Value {
    json!({ "mcpServers": { "post-cortex": server_entry(ctx, pcx) } })
}

fn install_into(path: &std::path::Path, ctx: &SetupContext, pcx: &str) -> Result<String, String> {
    let entry = transport_patch(server_entry(ctx, pcx));
    let change = merge_json(path, &json!({ "mcpServers": { "post-cortex": entry } }))?;
    let note = match change {
        Change::Unchanged => " (user-level)",
        _ => {
            " (user-level, Claude Desktop has no project config: restart Claude Desktop to load it)"
        }
    };
    Ok(format!("{}{note}", report(change, path, ctx)))
}

/// Merge the MCP entry into the user's `claude_desktop_config.json`.
pub fn install(ctx: &SetupContext) -> Result<Vec<String>, String> {
    let path = config_path().ok_or("Cannot find the OS config directory for Claude Desktop")?;
    Ok(vec![install_into(&path, ctx, &pcx_command())?])
}

/// The `mcpServers` entry for `claude_desktop_config.json`.
pub fn mcp_config(ctx: &SetupContext) -> String {
    let shown = config_path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "claude_desktop_config.json".into());
    let snippet = serde_json::to_string_pretty(&patch(ctx, &pcx_command())).unwrap_or_default();
    let note = if ctx.remote {
        REMOTE_TOKEN_NOTE
    } else {
        "The bearer token comes from daemon.toml (auth_token); PC_AUTH_TOKEN in your shell is not seen."
    };
    format!("// Merge into {shown}, then restart Claude Desktop.\n// {note}\n{snippet}\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clients::common::{cloud_args, remote};
    use std::path::Path;

    fn ctx(dir: &Path, url: &str) -> SetupContext {
        SetupContext {
            project_dir: dir.to_path_buf(),
            session_id: "S1".into(),
            workspace_id: "W1".into(),
            daemon_url: url.into(),
            capture: false,
            profile: "self".into(),
            remote: false,
            global: false,
        }
    }

    fn read_json(path: &Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn local_daemon_runs_pcx_stdio() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx(dir.path(), "http://127.0.0.1:3737");
        let path = dir.path().join("Claude/claude_desktop_config.json");
        let line = install_into(&path, &ctx, "/usr/local/bin/pcx").unwrap();
        assert!(line.starts_with("  Created: "), "{line}");
        assert!(line.contains("restart Claude Desktop"));
        let v = read_json(&path);
        let entry = &v["mcpServers"]["post-cortex"];
        assert_eq!(entry["command"], "/usr/local/bin/pcx");
        assert_eq!(entry["args"], json!([]));
        assert_eq!(entry["env"]["PC_HOST"], "127.0.0.1");
        assert_eq!(entry["env"]["PC_PORT"], "3737");
        assert_eq!(entry["env"]["PC_AUTHOR"], "claude-desktop");

        let before = std::fs::read(&path).unwrap();
        let again = install_into(&path, &ctx, "/usr/local/bin/pcx").unwrap();
        assert!(again.contains("already set up"), "{again}");
        assert!(!again.contains("restart"));
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn remote_profile_runs_pcx_mcp_without_a_token() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("claude_desktop_config.json");
        install_into(&path, &ctx(dir.path(), "http://127.0.0.1:3737"), "/bin/pcx").unwrap();
        let cloud = remote(ctx(dir.path(), "http://127.0.0.1:3737"));
        install_into(&path, &cloud, "/bin/pcx").unwrap();
        let v = read_json(&path);
        assert_eq!(
            v["mcpServers"]["post-cortex"],
            json!({ "command": "/bin/pcx", "args": cloud_args(Client::ClaudeDesktop) })
        );
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            !text.contains("PC_HOST") && !text.contains("Bearer"),
            "{text}"
        );
    }

    #[test]
    fn existing_config_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx(dir.path(), "http://localhost:4000");
        let path = dir.path().join("claude_desktop_config.json");
        std::fs::write(
            &path,
            r#"{"mcpServers":{"fs":{"command":"npx","args":["fs"]}},"globalShortcut":"Ctrl+Space"}"#,
        )
        .unwrap();
        assert!(
            install_into(&path, &ctx, "pcx")
                .unwrap()
                .starts_with("  Updated: ")
        );
        let v = read_json(&path);
        assert_eq!(v["mcpServers"]["fs"]["args"][0], "fs");
        assert_eq!(v["globalShortcut"], "Ctrl+Space");
        assert_eq!(
            v["mcpServers"]["post-cortex"]["env"]["PC_HOST"],
            "localhost"
        );
        assert_eq!(v["mcpServers"]["post-cortex"]["env"]["PC_PORT"], "4000");
    }

    #[test]
    fn remote_daemon_uses_mcp_remote() {
        let dir = tempfile::tempdir().unwrap();
        let https = server_entry(&ctx(dir.path(), "https://pcx.example.com"), "pcx");
        assert_eq!(https["command"], "npx");
        assert_eq!(
            https["args"],
            json!([
                "-y",
                "mcp-remote",
                "https://pcx.example.com/mcp",
                "--header",
                "X-PCX-Author: claude-desktop"
            ])
        );
        let lan = server_entry(&ctx(dir.path(), "http://10.0.0.5:3737"), "pcx");
        assert_eq!(lan["args"][5], "--allow-http");
    }

    #[test]
    fn host_port_parsing() {
        assert_eq!(
            local_host_port("http://127.0.0.1:3737/"),
            Some(("127.0.0.1".into(), 3737))
        );
        assert_eq!(local_host_port("http://[::1]:3737"), None);
        assert_eq!(local_host_port("http://127.0.0.1"), None);
        assert_eq!(local_host_port("https://127.0.0.1:3737"), None);
        assert_eq!(local_host_port("http://example.com:3737"), None);
    }

    #[test]
    fn mcp_config_is_a_comment_and_the_entry() {
        let dir = tempfile::tempdir().unwrap();
        let text = mcp_config(&ctx(dir.path(), "http://127.0.0.1:3737"));
        assert!(text.starts_with("// Merge into "));
        assert!(text.contains("claude_desktop_config.json"));
        let v: Value = serde_json::from_str(&text[text.find("\n{").unwrap()..]).unwrap();
        assert_eq!(
            v["mcpServers"]["post-cortex"]["env"]["PC_AUTHOR"],
            "claude-desktop"
        );
    }
}
