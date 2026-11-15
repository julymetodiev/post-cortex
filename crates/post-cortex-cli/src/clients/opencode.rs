// Copyright (c) 2026 Julius ML
// MIT License

//! OpenCode: the MCP entry in `opencode.json`, the instructions block in
//! `AGENTS.md`, the skill in `.opencode/skills` and a TypeScript plugin in
//! `.opencode/plugins/post-cortex.ts`.
//!
//! OpenCode runs no command hooks, so there is no `hook` here: the plugin
//! calls the daemon's REST routes itself. It puts the cards into the system
//! prompt, appends the file memory to the output of `edit`, `write` and
//! `apply_patch`, and (with capture) posts the session's turns when it goes
//! idle.

use serde_json::json;

use crate::clients::Client;
use crate::clients::common::{
    McpEntry, SetupContext, install_skill, merge_json_report, report, skipped, transport_patch,
    upsert_block, write_owned,
};

const PLUGIN: &str = include_str!("../../templates/clients/opencode/post-cortex.ts");

/// The `post-cortex` entry under `mcp` in `opencode.json`: for a remote
/// profile a local server (the pcx stdio proxy, `command` as one array),
/// else the daemon over HTTP, where `oauth: false` keeps OpenCode from
/// starting an OAuth flow when the daemon answers 401 (it takes a bearer
/// token, not OAuth).
fn mcp_entry(ctx: &SetupContext) -> serde_json::Value {
    match ctx.mcp_entry(Client::Opencode) {
        McpEntry::Stdio { command, args } => {
            let mut argv = vec![command];
            argv.extend(args);
            json!({ "type": "local", "command": argv, "enabled": true })
        }
        McpEntry::Http { url } => json!({
            "type": "remote",
            "url": url,
            "headers": {
                "Authorization": "Bearer {env:PC_AUTH_TOKEN}",
                "X-PCX-Author": Client::Opencode.author(),
            },
            "oauth": false,
            "enabled": true,
        }),
    }
}

/// [`mcp_entry`] as the install patch: the other transport's keys (and
/// `oauth`, which only a remote server has) are removed.
fn mcp_patch(ctx: &SetupContext) -> serde_json::Value {
    let mut entry = transport_patch(mcp_entry(ctx));
    entry
        .as_object_mut()
        .map(|map| map.entry("oauth").or_insert(serde_json::Value::Null));
    entry
}

/// The plugin with the daemon, session, workspace and capture flag filled in.
fn plugin(ctx: &SetupContext) -> String {
    PLUGIN
        .replace("<DAEMON_URL>", &ctx.daemon_url)
        .replace("<PROFILE>", &ctx.profile)
        .replace("<SESSION_ID>", &ctx.session_id)
        .replace("<WORKSPACE_ID>", &ctx.workspace_id)
        .replace("<CAPTURE>", if ctx.capture { "true" } else { "false" })
}

/// Write `opencode.json` (merged), the `AGENTS.md` block, the skill and the
/// plugin. In a project configured with `opencode.jsonc` only, the config
/// is skipped (its comments cannot be kept; `pcx mcp-config opencode`
/// prints the entry) and the rest is still written.
pub fn install(ctx: &SetupContext) -> Result<Vec<String>, String> {
    let config = ctx.project("opencode.json");
    let jsonc = ctx.project("opencode.jsonc");
    let mut lines = Vec::new();
    if !config.exists() && jsonc.exists() {
        lines.push(skipped(&format!(
            "{} is JSONC; add the post-cortex entry by hand (pcx mcp-config opencode prints it)",
            jsonc.display()
        )));
    } else {
        let patch = json!({
            "$schema": "https://opencode.ai/config.json",
            "mcp": { "post-cortex": mcp_patch(ctx) },
        });
        lines.push(merge_json_report(&config, &patch, ctx));
    }

    let agents = ctx.project("AGENTS.md");
    lines.push(report(
        upsert_block(&agents, &ctx.instructions())?,
        &agents,
        ctx,
    ));

    lines.extend(install_skill(&ctx.project(".opencode/skills"), ctx)?);

    let plugin_path = ctx.project(".opencode/plugins/post-cortex.ts");
    lines.push(report(
        write_owned(&plugin_path, &plugin(ctx), false)?,
        &plugin_path,
        ctx,
    ));
    Ok(lines)
}

/// The `mcp` entry to paste into `opencode.json` (or `opencode.jsonc`).
pub fn mcp_config(ctx: &SetupContext) -> String {
    let snippet = json!({ "mcp": { "post-cortex": mcp_entry(ctx) } });
    format!(
        "// opencode.json (project root) or ~/.config/opencode/opencode.json, merged into \"mcp\":\n{}\n",
        serde_json::to_string_pretty(&snippet).unwrap_or_default()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clients::common::remote;
    use serde_json::Value;
    use std::path::Path;

    fn ctx(dir: &Path, capture: bool) -> SetupContext {
        SetupContext {
            project_dir: dir.to_path_buf(),
            session_id: "S1".into(),
            workspace_id: "W1".into(),
            daemon_url: "http://127.0.0.1:3737".into(),
            capture,
            profile: "self".into(),
            remote: false,
            global: false,
        }
    }

    fn read_json(path: &Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn install_writes_config_agents_skill_and_plugin() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx(dir.path(), true);
        let lines = install(&ctx).unwrap();
        assert_eq!(lines.len(), 4);
        assert!(lines.iter().all(|l| l.contains("Created")), "{lines:?}");

        let config = read_json(&dir.path().join("opencode.json"));
        assert_eq!(config["$schema"], "https://opencode.ai/config.json");
        let entry = &config["mcp"]["post-cortex"];
        assert_eq!(entry["type"], "remote");
        assert_eq!(entry["url"], "http://127.0.0.1:3737/mcp");
        assert_eq!(
            entry["headers"]["Authorization"],
            "Bearer {env:PC_AUTH_TOKEN}"
        );
        assert_eq!(entry["headers"]["X-PCX-Author"], "opencode");
        assert_eq!(entry["oauth"], false);
        assert_eq!(entry["enabled"], true);

        let agents = std::fs::read_to_string(dir.path().join("AGENTS.md")).unwrap();
        assert!(agents.contains("S1") && agents.contains("W1"));
        assert!(
            dir.path()
                .join(".opencode/skills/post-cortex-memory/SKILL.md")
                .exists()
        );

        let ts =
            std::fs::read_to_string(dir.path().join(".opencode/plugins/post-cortex.ts")).unwrap();
        assert!(ts.contains(r#"|| "http://127.0.0.1:3737")"#));
        assert!(ts.contains(r#"const SESSION = "S1";"#));
        assert!(ts.contains(r#"const WORKSPACE = "W1";"#));
        assert!(ts.contains("const CAPTURE: boolean = true;"));
    }

    #[test]
    fn plugin_has_no_placeholders_left() {
        let dir = tempfile::tempdir().unwrap();
        for capture in [false, true] {
            let ts = plugin(&ctx(dir.path(), capture));
            for placeholder in [
                "<DAEMON_URL>",
                "<SESSION_ID>",
                "<WORKSPACE_ID>",
                "<CAPTURE>",
                "<PROFILE>",
            ] {
                assert!(!ts.contains(placeholder), "{placeholder} left");
            }
            assert!(ts.contains(&format!("const CAPTURE: boolean = {capture};")));
            assert!(ts.contains("export const PostCortex: Plugin"));
        }
    }

    #[test]
    fn remote_install_writes_a_local_server_and_no_token() {
        let dir = tempfile::tempdir().unwrap();
        install(&ctx(dir.path(), false)).unwrap();
        install(&remote(ctx(dir.path(), false))).unwrap();
        let text = std::fs::read_to_string(dir.path().join("opencode.json")).unwrap();
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            v["mcp"]["post-cortex"],
            json!({
                "type": "local",
                "command": ["pcx", "mcp", "--profile", "cloud", "--author", "opencode"],
                "enabled": true,
            })
        );
        assert!(!text.contains("PC_AUTH_TOKEN"), "{text}");
        let plugin =
            std::fs::read_to_string(dir.path().join(".opencode/plugins/post-cortex.ts")).unwrap();
        assert!(plugin.contains("const PROFILE = \"cloud\";"));
        assert!(plugin.contains("https://api.pcx.is"));
    }

    #[test]
    fn second_install_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ctx(dir.path(), false);
        install(&ctx).unwrap();
        let files = [
            "opencode.json",
            "AGENTS.md",
            ".opencode/skills/post-cortex-memory/SKILL.md",
            ".opencode/plugins/post-cortex.ts",
        ];
        let before: Vec<Vec<u8>> = files
            .iter()
            .map(|f| std::fs::read(dir.path().join(f)).unwrap())
            .collect();
        let lines = install(&ctx).unwrap();
        assert!(
            lines.iter().all(|l| l.contains("already set up")),
            "{lines:?}"
        );
        for (f, old) in files.iter().zip(before) {
            assert_eq!(std::fs::read(dir.path().join(f)).unwrap(), old, "{f}");
        }
    }

    #[test]
    fn existing_config_and_agents_md_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("opencode.json"),
            r#"{"model":"x/y","mcp":{"other":{"type":"local","command":["z"]}}}"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "# Mine\n\nKeep me.\n").unwrap();
        let lines = install(&ctx(dir.path(), false)).unwrap();
        assert!(lines[0].contains("Updated") && lines[1].contains("Updated"));
        let config = read_json(&dir.path().join("opencode.json"));
        assert_eq!(config["model"], "x/y");
        assert_eq!(config["mcp"]["other"]["command"][0], "z");
        assert_eq!(config["mcp"]["post-cortex"]["type"], "remote");
        let agents = std::fs::read_to_string(dir.path().join("AGENTS.md")).unwrap();
        assert!(agents.starts_with("# Mine\n\nKeep me.\n"));
    }

    #[test]
    fn jsonc_only_project_is_skipped_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let jsonc = dir.path().join("opencode.jsonc");
        std::fs::write(&jsonc, "{ // mine\n}\n").unwrap();
        let lines = install(&ctx(dir.path(), false)).unwrap();
        assert!(
            lines[0].starts_with("  Skipped: ") && lines[0].contains("opencode.jsonc"),
            "{lines:?}"
        );
        assert_eq!(std::fs::read_to_string(&jsonc).unwrap(), "{ // mine\n}\n");
        assert!(!dir.path().join("opencode.json").exists());
        assert!(dir.path().join(".opencode/plugins/post-cortex.ts").exists());
    }

    #[test]
    fn mcp_config_names_the_file_and_parses() {
        let dir = tempfile::tempdir().unwrap();
        let text = mcp_config(&ctx(dir.path(), false));
        let (comment, body) = text.split_once('\n').unwrap();
        assert!(comment.starts_with("//") && comment.contains("opencode.json"));
        let v: Value = serde_json::from_str(body).unwrap();
        assert_eq!(v["mcp"]["post-cortex"]["url"], "http://127.0.0.1:3737/mcp");
    }
}
