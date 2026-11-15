// Copyright (c) 2026 Julius ML
// MIT License

//! Goose: instructions, and a check of the MCP extension.
//!
//! - MCP: Goose keeps extensions in the user-level `config.yaml`
//!   (`~/.config/goose/config.yaml`, `$XDG_CONFIG_HOME/goose` when set,
//!   `%APPDATA%\Block\goose\config` on Windows). Setup does not rewrite YAML
//!   (no YAML parser here, and the file holds the user's own settings): it
//!   reports whether a `post-cortex` extension is there and, if not, says
//!   to paste `pcx mcp-config goose` (or add it with `goose configure`).
//!   Headers expand `${VAR}` only for variables listed in `env_keys`, and a
//!   listed variable that is unset stops the extension from loading, so the
//!   bearer token lines are printed commented out.
//! - Instructions: the root `AGENTS.md` (Goose reads it, and `.goosehints`,
//!   by default).
//! - Hooks: Goose runs no hooks, so setup installs none and `hook` returns
//!   `None` for every event.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::clients::Client;
use crate::clients::common::{
    Change, HookEnv, McpEntry, REMOTE_TOKEN_NOTE, SetupContext, report, report_path, upsert_block,
    user_config_dir,
};

pub fn install(ctx: &SetupContext) -> Result<Vec<String>, String> {
    let config = config_file().ok_or("Cannot find the home directory")?;
    install_at(ctx, &config)
}

/// `install` with the Goose config file given (tests use a temp dir).
fn install_at(ctx: &SetupContext, config: &Path) -> Result<Vec<String>, String> {
    let agents = ctx.project("AGENTS.md");
    let mut lines = vec![report(
        upsert_block(&agents, &ctx.instructions())?,
        &agents,
        ctx,
    )];
    let text = match std::fs::read_to_string(config) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("Failed to read {}: {e}", config.display())),
    };
    lines.push(if has_extension(&text, "post-cortex") {
        format!(
            "{} (user-level: post-cortex extension found)",
            report(Change::Unchanged, config, ctx)
        )
    } else {
        format!(
            "  MCP: {} has no post-cortex extension; add it with `goose configure` or paste `pcx mcp-config goose` under `extensions:` (user-level)",
            report_path(config, ctx)
        )
    });
    Ok(lines)
}

pub fn mcp_config(ctx: &SetupContext) -> String {
    if let McpEntry::Stdio { command, args } = ctx.mcp_entry(Client::Goose) {
        let args: Vec<String> = args.iter().map(|a| yaml_str(a)).collect();
        return format!(
            "# Add under `extensions:` in ~/.config/goose/config.yaml\n\
             # (Windows: %APPDATA%\\Block\\goose\\config\\config.yaml)\n\
             # {REMOTE_TOKEN_NOTE}\n\
             \x20 post-cortex:\n\
             \x20   enabled: true\n\
             \x20   type: stdio\n\
             \x20   name: post-cortex\n\
             \x20   cmd: {}\n\
             \x20   args: [{}]\n\
             \x20   timeout: 300\n",
            yaml_str(&command),
            args.join(", ")
        );
    }
    format!(
        "# Add under `extensions:` in ~/.config/goose/config.yaml\n\
         # (Windows: %APPDATA%\\Block\\goose\\config\\config.yaml)\n\
         \x20 post-cortex:\n\
         \x20   enabled: true\n\
         \x20   type: streamable_http\n\
         \x20   name: post-cortex\n\
         \x20   uri: {}\n\
         \x20   timeout: 300\n\
         \x20   headers:\n\
         \x20     X-PCX-Author: {}\n\
         # If the daemon has an auth_token (PC_AUTH_TOKEN must then be set, or the extension fails to load):\n\
         #     Authorization: Bearer ${{PC_AUTH_TOKEN}}\n\
         #   env_keys:\n\
         #     - PC_AUTH_TOKEN\n",
        ctx.mcp_url(),
        Client::Goose.author()
    )
}

/// `s` as a double-quoted YAML scalar (a path may hold spaces or colons).
fn yaml_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_default()
}

/// Goose has no hooks: nothing to answer.
pub async fn hook(_event: &str, _input: Value, _env: &HookEnv) -> Result<Option<String>, String> {
    Ok(None)
}

/// Goose's `config.yaml`.
fn config_file() -> Option<PathBuf> {
    let rel = if cfg!(windows) {
        "Block/goose/config/config.yaml"
    } else {
        "goose/config.yaml"
    };
    Some(user_config_dir()?.join(rel))
}

/// Whether the top-level `extensions:` mapping of a Goose config has a
/// `name:` key as a direct child (a line scan: block-style YAML, which is
/// what Goose writes).
fn has_extension(yaml: &str, name: &str) -> bool {
    let mut in_extensions = false;
    let mut child_indent: Option<usize> = None;
    for line in yaml.lines() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = line.len() - trimmed.len();
        if indent == 0 {
            in_extensions = trimmed.trim_end() == "extensions:";
            child_indent = None;
            continue;
        }
        if !in_extensions {
            continue;
        }
        let level = *child_indent.get_or_insert(indent);
        if indent != level {
            continue;
        }
        let key = trimmed
            .split_once(':')
            .map(|(k, _)| k.trim().trim_matches(['"', '\'']));
        if key == Some(name) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(dir: &Path) -> SetupContext {
        SetupContext {
            project_dir: dir.to_path_buf(),
            session_id: "S1".into(),
            workspace_id: "W1".into(),
            daemon_url: "http://127.0.0.1:3737".into(),
            capture: true,
            profile: "self".into(),
            remote: false,
            global: false,
        }
    }

    #[test]
    fn install_writes_agents_md_and_never_touches_yaml() {
        let project = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let config = home.path().join("config.yaml");
        let yaml = "GOOSE_PROVIDER: openai\nextensions:\n  developer:\n    enabled: true\n    type: builtin\n";
        std::fs::write(&config, yaml).unwrap();
        let ctx = ctx(project.path());

        let lines = install_at(&ctx, &config).unwrap();
        assert!(lines[0].contains("Created: AGENTS.md"));
        assert!(
            lines[1].contains("no post-cortex extension")
                && lines[1].contains("pcx mcp-config goose")
        );
        assert_eq!(std::fs::read_to_string(&config).unwrap(), yaml);
        assert!(!project.path().join(".goose").exists());

        let before = std::fs::read(project.path().join("AGENTS.md")).unwrap();
        let again = install_at(&ctx, &config).unwrap();
        assert!(again[0].contains("already set up"));
        assert_eq!(
            std::fs::read(project.path().join("AGENTS.md")).unwrap(),
            before
        );

        std::fs::write(&config, format!("{yaml}{}", mcp_config(&ctx))).unwrap();
        let found = install_at(&ctx, &config).unwrap();
        assert!(
            found[1].contains("post-cortex extension found"),
            "{found:?}"
        );
    }

    #[test]
    fn missing_config_reports_how_to_add() {
        let project = tempfile::tempdir().unwrap();
        let lines = install_at(&ctx(project.path()), &project.path().join("nope.yaml")).unwrap();
        assert!(lines[1].contains("goose configure"));
    }

    #[test]
    fn has_extension_reads_only_direct_children_of_extensions() {
        let yaml =
            "extensions:\n  memory:\n    name: post-cortex\n  'post-cortex':\n    enabled: true\n";
        assert!(has_extension(yaml, "post-cortex"));
        let nested = "extensions:\n  memory:\n    post-cortex: x\nother:\n  post-cortex: y\n";
        assert!(!has_extension(nested, "post-cortex"));
        assert!(!has_extension("post-cortex:\n  a: b\n", "post-cortex"));
        assert!(!has_extension("", "post-cortex"));
    }

    #[test]
    fn mcp_config_is_an_extensions_entry() {
        let dir = tempfile::tempdir().unwrap();
        let text = mcp_config(&ctx(dir.path()));
        assert!(text.starts_with("# Add under `extensions:` in ~/.config/goose/config.yaml"));
        assert!(text.contains("  post-cortex:\n    enabled: true\n    type: streamable_http\n"));
        assert!(text.contains("    uri: http://127.0.0.1:3737/mcp\n"));
        assert!(text.contains("      X-PCX-Author: goose\n"));
        assert!(has_extension(
            &format!("extensions:\n{text}"),
            "post-cortex"
        ));
    }

    #[test]
    fn remote_mcp_config_is_a_stdio_extension_without_a_token() {
        let dir = tempfile::tempdir().unwrap();
        let text = mcp_config(&crate::clients::common::remote(ctx(dir.path())));
        assert!(text.contains("    type: stdio\n"), "{text}");
        assert!(text.contains("    cmd: \"pcx\"\n"), "{text}");
        assert!(
            text.contains(r#"    args: ["mcp", "--profile", "cloud", "--author", "goose"]"#),
            "{text}"
        );
        assert!(
            !text.contains("PC_AUTH_TOKEN") && !text.contains("uri:"),
            "{text}"
        );
    }

    #[tokio::test]
    async fn hook_is_a_no_op() {
        let env = HookEnv::new(
            "http://127.0.0.1:9",
            reqwest::Client::new(),
            "S1",
            "W1",
            "goose",
        );
        assert_eq!(hook("session-start", Value::Null, &env).await, Ok(None));
    }
}
