// Copyright (c) 2026 Julius ML
// MIT License

//! Zed: the MCP entry under `context_servers` in the project settings
//! (`.zed/settings.json`) and the instructions block in AGENTS.md. Zed runs
//! no hooks.
//!
//! Zed reads one project instructions file, the first it finds of
//! [`RULES_BEFORE_AGENTS`], then `AGENTS.md`, `CLAUDE.md`, `GEMINI.md`; so
//! AGENTS.md wins over the CLAUDE.md Claude Code setup writes. Zed settings
//! are JSONC: a settings file with comments is not merged, setup says so and
//! `pcx mcp-config zed` prints the entry to paste.

use serde_json::{Value, json};

use crate::clients::Client;
use crate::clients::common::{
    McpEntry, REMOTE_TOKEN_NOTE, SetupContext, merge_json_report, report, transport_patch,
    upsert_block,
};

/// The project settings file.
const SETTINGS: &str = ".zed/settings.json";

/// Project instructions files Zed reads instead of AGENTS.md when present.
const RULES_BEFORE_AGENTS: &[&str] = &[
    ".rules",
    ".cursorrules",
    ".windsurfrules",
    ".clinerules",
    ".github/copilot-instructions.md",
    "AGENT.md",
];

/// The `context_servers` entry: the pcx stdio proxy for a remote profile,
/// else the daemon over HTTP (Zed has no env interpolation in headers, so
/// no bearer token there).
fn server_entry(ctx: &SetupContext) -> Value {
    match ctx.mcp_entry(Client::Zed) {
        McpEntry::Stdio { command, args } => json!({ "command": command, "args": args }),
        McpEntry::Http { url } => json!({
            "url": url,
            "headers": { "X-PCX-Author": Client::Zed.author() },
        }),
    }
}

fn settings_patch(ctx: &SetupContext) -> Value {
    json!({ "context_servers": { "post-cortex": server_entry(ctx) } })
}

/// The first project file Zed reads instead of AGENTS.md, if any.
fn shadowing_rules_file(ctx: &SetupContext) -> Option<&'static str> {
    RULES_BEFORE_AGENTS
        .iter()
        .copied()
        .find(|rel| ctx.project(rel).is_file())
}

/// Write the MCP entry into `.zed/settings.json` and the instructions block
/// into AGENTS.md. A settings file that is not plain JSON is reported as
/// skipped and left untouched; AGENTS.md is still written.
pub fn install(ctx: &SetupContext) -> Result<Vec<String>, String> {
    let mut lines = Vec::new();
    let settings = ctx.project(SETTINGS);
    let patch = json!({ "context_servers": { "post-cortex": transport_patch(server_entry(ctx)) } });
    lines.push(merge_json_report(&settings, &patch, ctx));
    let agents = ctx.project("AGENTS.md");
    let mut line = report(upsert_block(&agents, &ctx.instructions())?, &agents, ctx);
    if let Some(first) = shadowing_rules_file(ctx) {
        line.push_str(&format!(
            " (Zed reads {first} instead, it comes first in Zed's list: copy the post-cortex block there)"
        ));
    }
    lines.push(line);
    Ok(lines)
}

/// The `context_servers` entry for `.zed/settings.json`.
pub fn mcp_config(ctx: &SetupContext) -> String {
    let snippet = serde_json::to_string_pretty(&settings_patch(ctx)).unwrap_or_default();
    let note = if ctx.remote {
        format!("// {REMOTE_TOKEN_NOTE}\n")
    } else {
        "// Zed does not expand env vars in headers: if the daemon has an auth_token, add\n\
         // \"Authorization\": \"Bearer <token>\" to headers.\n"
            .to_string()
    };
    format!(
        "// Merge into .zed/settings.json (project) or ~/.config/zed/settings.json (user).\n{note}{snippet}\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clients::common::{BLOCK_BEGIN, BLOCK_END, cloud_args, remote};
    use std::path::Path;

    fn ctx(dir: &Path) -> SetupContext {
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

    fn read_json(path: &Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn install_writes_settings_and_agents_md_once() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx(dir.path());
        let lines = install(&ctx).unwrap();
        assert_eq!(
            lines,
            vec![
                "  Created: .zed/settings.json".to_string(),
                "  Created: AGENTS.md".to_string()
            ]
        );
        let v = read_json(&ctx.project(SETTINGS));
        let entry = &v["context_servers"]["post-cortex"];
        assert_eq!(entry["url"], "http://127.0.0.1:3737/mcp");
        assert_eq!(entry["headers"]["X-PCX-Author"], "zed");
        assert!(entry["headers"].get("Authorization").is_none());
        let agents = std::fs::read_to_string(ctx.project("AGENTS.md")).unwrap();
        assert!(agents.starts_with(BLOCK_BEGIN) && agents.contains(BLOCK_END));
        assert!(agents.contains("`S1`") && agents.contains("`W1`"));

        let before_settings = std::fs::read(ctx.project(SETTINGS)).unwrap();
        let before_agents = std::fs::read(ctx.project("AGENTS.md")).unwrap();
        let again = install(&ctx).unwrap();
        assert!(
            again.iter().all(|l| l.contains("already set up")),
            "{again:?}"
        );
        assert_eq!(
            std::fs::read(ctx.project(SETTINGS)).unwrap(),
            before_settings
        );
        assert_eq!(
            std::fs::read(ctx.project("AGENTS.md")).unwrap(),
            before_agents
        );
    }

    #[test]
    fn remote_install_writes_the_stdio_entry_and_no_token() {
        let dir = tempfile::tempdir().unwrap();
        install(&ctx(dir.path())).unwrap();
        install(&remote(ctx(dir.path()))).unwrap();
        let text = std::fs::read_to_string(dir.path().join(SETTINGS)).unwrap();
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            v["context_servers"]["post-cortex"],
            json!({ "command": "pcx", "args": cloud_args(Client::Zed) })
        );
        assert!(
            !text.contains("Bearer") && !text.contains("\"url\""),
            "{text}"
        );
    }

    #[test]
    fn install_keeps_user_settings() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx(dir.path());
        std::fs::create_dir_all(dir.path().join(".zed")).unwrap();
        std::fs::write(
            ctx.project(SETTINGS),
            r#"{"tab_size":2,"context_servers":{"other":{"url":"http://x"}}}"#,
        )
        .unwrap();
        std::fs::write(ctx.project("AGENTS.md"), "# Mine\n").unwrap();
        let lines = install(&ctx).unwrap();
        assert_eq!(lines[0], "  Updated: .zed/settings.json");
        let v = read_json(&ctx.project(SETTINGS));
        assert_eq!(v["tab_size"], 2);
        assert_eq!(v["context_servers"]["other"]["url"], "http://x");
        assert_eq!(
            v["context_servers"]["post-cortex"]["headers"]["X-PCX-Author"],
            "zed"
        );
        let agents = std::fs::read_to_string(ctx.project("AGENTS.md")).unwrap();
        assert!(agents.starts_with("# Mine\n\n"));
    }

    #[test]
    fn jsonc_settings_are_skipped_and_agents_md_still_written() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx(dir.path());
        std::fs::create_dir_all(dir.path().join(".zed")).unwrap();
        let jsonc = "// Folder-specific settings\n{\n  \"tab_size\": 2,\n}\n";
        std::fs::write(ctx.project(SETTINGS), jsonc).unwrap();
        let lines = install(&ctx).unwrap();
        assert!(lines[0].starts_with("  Skipped: "), "{lines:?}");
        assert!(lines[0].contains("pcx mcp-config"));
        assert_eq!(
            std::fs::read_to_string(ctx.project(SETTINGS)).unwrap(),
            jsonc
        );
        assert_eq!(lines[1], "  Created: AGENTS.md");
    }

    #[test]
    fn a_rules_file_ahead_of_agents_md_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx(dir.path());
        std::fs::write(ctx.project(".rules"), "be nice\n").unwrap();
        let lines = install(&ctx).unwrap();
        assert!(lines[1].contains("Zed reads .rules instead"), "{lines:?}");
        assert_eq!(
            std::fs::read_to_string(ctx.project(".rules")).unwrap(),
            "be nice\n"
        );

        let plain = tempfile::tempdir().unwrap();
        std::fs::write(plain.path().join("CLAUDE.md"), "x\n").unwrap();
        let ctx = self::ctx(plain.path());
        let lines = install(&ctx).unwrap();
        assert_eq!(lines[1], "  Created: AGENTS.md");
    }

    #[test]
    fn mcp_config_is_a_comment_and_the_entry() {
        let dir = tempfile::tempdir().unwrap();
        let text = mcp_config(&ctx(dir.path()));
        assert!(text.starts_with("// Merge into .zed/settings.json"));
        let json_start = text.find('{').unwrap();
        let v: Value = serde_json::from_str(&text[json_start..]).unwrap();
        assert_eq!(
            v["context_servers"]["post-cortex"]["url"],
            "http://127.0.0.1:3737/mcp"
        );
    }
}
