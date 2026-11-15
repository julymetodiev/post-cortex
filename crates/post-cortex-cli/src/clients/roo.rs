// Copyright (c) 2026 Julius ML
// MIT License

//! Roo Code: the MCP entry in the project config (`.roo/mcp.json`) and the
//! instructions block in AGENTS.md, which Roo loads by default (setting
//! `useAgentRules`). Roo runs no hooks.
//!
//! Roo expands `${env:NAME}` anywhere in a server entry (it injects the
//! variables into the whole serialized config), so the bearer token comes
//! from `PC_AUTH_TOKEN`. An unset variable stays literal; a daemon without
//! an auth_token ignores the header.

use serde_json::{Value, json};

use crate::clients::Client;
use crate::clients::common::{
    McpEntry, REMOTE_TOKEN_NOTE, SetupContext, merge_json_report, report, transport_patch,
    upsert_block,
};

/// The project MCP config.
const MCP_JSON: &str = ".roo/mcp.json";

/// The `post-cortex` entry: the pcx stdio proxy for a remote profile, else
/// the daemon over Streamable HTTP.
fn server_entry(ctx: &SetupContext) -> Value {
    match ctx.mcp_entry(Client::Roo) {
        McpEntry::Stdio { command, args } => {
            json!({ "type": "stdio", "command": command, "args": args })
        }
        McpEntry::Http { url } => json!({
            "type": "streamable-http",
            "url": url,
            "headers": {
                "X-PCX-Author": Client::Roo.author(),
                "Authorization": "Bearer ${env:PC_AUTH_TOKEN}",
            },
        }),
    }
}

fn mcp_patch(ctx: &SetupContext) -> Value {
    json!({ "mcpServers": { "post-cortex": server_entry(ctx) } })
}

/// Write the MCP entry into `.roo/mcp.json` and the instructions block into
/// AGENTS.md. A config that is not plain JSON is reported as skipped and
/// left untouched; AGENTS.md is still written.
pub fn install(ctx: &SetupContext) -> Result<Vec<String>, String> {
    let mut lines = Vec::new();
    let mcp = ctx.project(MCP_JSON);
    let patch = json!({ "mcpServers": { "post-cortex": transport_patch(server_entry(ctx)) } });
    lines.push(merge_json_report(&mcp, &patch, ctx));
    let agents = ctx.project("AGENTS.md");
    lines.push(report(
        upsert_block(&agents, &ctx.instructions())?,
        &agents,
        ctx,
    ));
    Ok(lines)
}

/// The `mcpServers` entry for `.roo/mcp.json`.
pub fn mcp_config(ctx: &SetupContext) -> String {
    let snippet = serde_json::to_string_pretty(&mcp_patch(ctx)).unwrap_or_default();
    let note = if ctx.remote {
        REMOTE_TOKEN_NOTE
    } else {
        "Roo expands ${env:PC_AUTH_TOKEN} from the editor's environment."
    };
    format!(
        "// Merge into .roo/mcp.json (project) or Roo's global mcp_settings.json.\n// {note}\n{snippet}\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clients::common::{cloud_args, remote};
    use std::path::Path;

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

    fn read_json(path: &Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn install_writes_mcp_json_and_agents_md_once() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx(dir.path());
        assert_eq!(
            install(&ctx).unwrap(),
            vec![
                "  Created: .roo/mcp.json".to_string(),
                "  Created: AGENTS.md".to_string()
            ]
        );
        let v = read_json(&ctx.project(MCP_JSON));
        let entry = &v["mcpServers"]["post-cortex"];
        assert_eq!(entry["type"], "streamable-http");
        assert_eq!(entry["url"], "http://127.0.0.1:3737/mcp");
        assert_eq!(entry["headers"]["X-PCX-Author"], "roo-code");
        assert_eq!(
            entry["headers"]["Authorization"],
            "Bearer ${env:PC_AUTH_TOKEN}"
        );
        assert!(
            std::fs::read_to_string(ctx.project("AGENTS.md"))
                .unwrap()
                .contains("`S1`")
        );
        // No hooks and no rules file of its own.
        assert!(!dir.path().join(".roo/rules").exists());

        let before = std::fs::read(ctx.project(MCP_JSON)).unwrap();
        let again = install(&ctx).unwrap();
        assert!(
            again.iter().all(|l| l.contains("already set up")),
            "{again:?}"
        );
        assert_eq!(std::fs::read(ctx.project(MCP_JSON)).unwrap(), before);
    }

    #[test]
    fn remote_install_writes_the_stdio_entry_and_no_token() {
        let dir = tempfile::tempdir().unwrap();
        install(&ctx(dir.path())).unwrap();
        install(&remote(ctx(dir.path()))).unwrap();
        let text = std::fs::read_to_string(dir.path().join(MCP_JSON)).unwrap();
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            v["mcpServers"]["post-cortex"],
            json!({ "type": "stdio", "command": "pcx", "args": cloud_args(Client::Roo) })
        );
        assert!(!text.contains("PC_AUTH_TOKEN"), "{text}");
    }

    #[test]
    fn install_keeps_other_servers() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx(dir.path());
        std::fs::create_dir_all(dir.path().join(".roo")).unwrap();
        std::fs::write(
            ctx.project(MCP_JSON),
            r#"{"mcpServers":{"github":{"command":"gh-mcp","alwaysAllow":["x"]}}}"#,
        )
        .unwrap();
        assert_eq!(install(&ctx).unwrap()[0], "  Updated: .roo/mcp.json");
        let v = read_json(&ctx.project(MCP_JSON));
        assert_eq!(v["mcpServers"]["github"]["command"], "gh-mcp");
        assert_eq!(v["mcpServers"]["github"]["alwaysAllow"][0], "x");
        assert_eq!(v["mcpServers"]["post-cortex"]["type"], "streamable-http");
    }

    #[test]
    fn broken_mcp_json_is_skipped_and_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx(dir.path());
        std::fs::create_dir_all(dir.path().join(".roo")).unwrap();
        std::fs::write(ctx.project(MCP_JSON), "{ // mine\n}").unwrap();
        let lines = install(&ctx).unwrap();
        assert!(lines[0].starts_with("  Skipped: "), "{lines:?}");
        assert_eq!(
            std::fs::read_to_string(ctx.project(MCP_JSON)).unwrap(),
            "{ // mine\n}"
        );
        assert_eq!(lines[1], "  Created: AGENTS.md");
    }

    #[test]
    fn mcp_config_is_a_comment_and_the_entry() {
        let dir = tempfile::tempdir().unwrap();
        let text = mcp_config(&ctx(dir.path()));
        assert!(text.starts_with("// Merge into .roo/mcp.json"));
        assert!(text.contains("${env:PC_AUTH_TOKEN} from"));
        let v: Value = serde_json::from_str(&text[text.find("\n{").unwrap()..]).unwrap();
        assert_eq!(v["mcpServers"]["post-cortex"]["type"], "streamable-http");
    }
}
