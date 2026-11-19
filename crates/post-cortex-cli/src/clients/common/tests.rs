// Copyright (c) 2026 Julius ML
// MIT License

use super::*;

mod file_memory;
use serde_json::{Value, json};

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

fn cloud(dir: &Path) -> SetupContext {
    SetupContext {
        daemon_url: "https://api.pcx.is".into(),
        profile: "cloud".into(),
        remote: true,
        ..ctx(dir)
    }
}

#[test]
fn upsert_block_appends_then_replaces_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("AGENTS.md");
    std::fs::write(&path, "# Project\n\nOwn rules.\n").unwrap();
    assert_eq!(upsert_block(&path, "one").unwrap(), Change::Updated);
    assert_eq!(upsert_block(&path, "one").unwrap(), Change::Unchanged);
    std::fs::write(
        &path,
        format!("{}\nAfter.\n", std::fs::read_to_string(&path).unwrap()),
    )
    .unwrap();
    assert_eq!(upsert_block(&path, "two").unwrap(), Change::Updated);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.starts_with("# Project\n\nOwn rules.\n"));
    assert!(text.contains(&format!("{BLOCK_BEGIN}\ntwo\n{BLOCK_END}\n")));
    assert!(text.ends_with("After.\n"));
    assert!(!text.contains("one"));

    let fresh = dir.path().join("new.md");
    assert_eq!(upsert_block(&fresh, "x").unwrap(), Change::Created);
    let _ = ctx(dir.path());
}

#[test]
fn merge_json_keeps_other_keys_and_refuses_jsonc() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mcp.json");
    std::fs::write(
        &path,
        r#"{"mcpServers":{"other":{"command":"x"}},"theme":"dark"}"#,
    )
    .unwrap();
    let patch = serde_json::json!({"mcpServers": {"post-cortex": {"url": "u"}}});
    assert_eq!(merge_json(&path, &patch).unwrap(), Change::Updated);
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(v["mcpServers"]["other"]["command"], "x");
    assert_eq!(v["mcpServers"]["post-cortex"]["url"], "u");
    assert_eq!(v["theme"], "dark");
    assert_eq!(merge_json(&path, &patch).unwrap(), Change::Unchanged);

    let jsonc = dir.path().join("c.json");
    std::fs::write(&jsonc, "{ // comment\n}").unwrap();
    assert!(merge_json(&jsonc, &patch).is_err());
    assert_eq!(std::fs::read_to_string(&jsonc).unwrap(), "{ // comment\n}");
}

#[test]
fn toml_section_is_appended_then_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "# mine\nmodel = \"x\"\n").unwrap();
    let header = "[mcp_servers.post-cortex]";
    let section = "[mcp_servers.post-cortex]\nurl = \"u\"\n";
    assert_eq!(
        upsert_toml_section(&path, header, section).unwrap(),
        Change::Updated
    );
    assert_eq!(
        upsert_toml_section(&path, header, section).unwrap(),
        Change::Unchanged
    );
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        text,
        "# mine\nmodel = \"x\"\n\n[mcp_servers.post-cortex]\nurl = \"u\"\n"
    );

    // Another backend's section, with a subtable, between the person's
    // own sections: replaced, the rest kept.
    std::fs::write(
        &path,
        "# mine\n[a]\nx = 1\n\n[mcp_servers.post-cortex]\ncommand = \"pcx\"\n\n[mcp_servers.post-cortex.env]\nPC_PORT = \"1\"\n\n# theirs\n[mcp_servers.other]\nurl = \"o\"\n",
    )
    .unwrap();
    assert_eq!(
        upsert_toml_section(&path, header, section).unwrap(),
        Change::Updated
    );
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "# mine\n[a]\nx = 1\n\n[mcp_servers.post-cortex]\nurl = \"u\"\n\n# theirs\n[mcp_servers.other]\nurl = \"o\"\n"
    );
}

#[test]
fn merge_json_report_skips_a_refused_file() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx(dir.path());
    let path = dir.path().join("a/mcp.json");
    let patch = json!({"x": 1});
    assert_eq!(
        merge_json_report(&path, &patch, &ctx),
        "  Created: a/mcp.json"
    );
    assert_eq!(report_path(&path, &ctx), "a/mcp.json");
    std::fs::write(&path, "{ // mine\n}").unwrap();
    let line = merge_json_report(&path, &patch, &ctx);
    assert!(
        line.starts_with("  Skipped: ") && line.contains("not plain JSON"),
        "{line}"
    );
    assert_eq!(json_file(&path), Value::Null);
}

#[test]
fn hook_command_names_the_profile() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        ctx(dir.path()).hook_command(Client::Cursor, "stop"),
        "pcx hook cursor stop --session S1 --workspace W1 --profile self --url http://127.0.0.1:3737"
    );
    let remote = cloud(dir.path()).hook_command(Client::Cursor, "stop");
    assert_eq!(
        remote,
        "pcx hook cursor stop --session S1 --workspace W1 --profile cloud"
    );
    assert!(!remote.contains("--url"));
}

#[test]
fn mcp_entry_is_stdio_for_a_remote_profile() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        ctx(dir.path()).mcp_entry(Client::Zed),
        McpEntry::Http {
            url: "http://127.0.0.1:3737/mcp".into()
        }
    );
    assert_eq!(
        cloud(dir.path()).mcp_entry(Client::Zed),
        McpEntry::Stdio {
            command: "pcx".into(),
            args: ["mcp", "--profile", "cloud", "--author", "zed"]
                .map(String::from)
                .to_vec(),
        }
    );
}

#[test]
fn transport_patch_switches_an_http_entry_to_stdio() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mcp.json");
    std::fs::write(
        &path,
        r#"{"mcpServers":{"post-cortex":{"url":"u","headers":{"A":"b"},"disabled":false}}}"#,
    )
    .unwrap();
    let entry = transport_patch(json!({ "command": "pcx", "args": ["mcp"] }));
    merge_json(&path, &json!({ "mcpServers": { "post-cortex": entry } })).unwrap();
    let v = json_file(&path);
    assert_eq!(
        v["mcpServers"]["post-cortex"],
        json!({ "command": "pcx", "args": ["mcp"], "disabled": false })
    );
}

#[test]
fn our_hooks_are_recognised_by_any_pcx_path() {
    let ours = |c: &str| is_our_hook(&json!({ "command": c }), Client::Cursor);
    assert!(ours("pcx hook cursor stop --session S"));
    assert!(ours("/usr/local/bin/pcx hook cursor stop"));
    assert!(ours("\"/Users/a b/bin/pcx\" hook cursor stop"));
    assert!(ours("C:/tools/pcx.exe hook cursor stop"));
    assert!(!ours("pcx hook codex stop"));
    assert!(!ours("echo hook cursor stop"));
    assert!(!ours("./mine.sh"));
    assert!(!is_our_hook(&json!({ "hooks": [] }), Client::Cursor));
}

#[test]
fn hook_arrays_keep_the_users_entries_and_replace_ours() {
    let old = "pcx hook codex stop --session OLD";
    let new = "/opt/bin/pcx hook codex stop --session NEW";
    // Claude-style groups: the person's hook in a mixed group stays.
    let existing = json!({
        "Stop": [
            { "hooks": [{ "command": "mine.sh" }, { "command": old }] },
            { "matcher": "x", "hooks": [{ "command": old }] },
            { "matcher": "no hooks list" },
        ],
        "SessionStart": [{ "hooks": [{ "command": old }] }],
        "Other": [{ "hooks": [{ "command": old }] }],
    });
    let ours = vec![json!({ "hooks": [{ "command": new }] })];
    let out = hook_arrays(
        &existing,
        Client::Codex,
        vec![
            ("Stop", ours.clone()),
            ("SessionStart", vec![]),
            ("Absent", vec![]),
        ],
    );
    assert_eq!(
        out,
        json!({
            "Stop": [
                { "hooks": [{ "command": "mine.sh" }] },
                { "matcher": "no hooks list" },
                { "hooks": [{ "command": new }] },
            ],
            "SessionStart": [],
        })
    );

    // Flat entries, nothing there yet.
    let flat = vec![json!({ "command": new })];
    assert_eq!(
        hook_arrays(
            &Value::Null,
            Client::Codex,
            vec![("stop", flat.clone()), ("x", vec![])]
        ),
        json!({ "stop": flat })
    );
    let existing = json!({ "stop": [{ "command": "./a.sh" }, { "command": old }] });
    assert_eq!(
        hook_arrays(&existing, Client::Codex, vec![("stop", flat)]),
        json!({ "stop": [{ "command": "./a.sh" }, { "command": new }] })
    );
}

#[test]
fn patch_paths_come_from_the_file_headers() {
    let patch = "*** Begin Patch\n*** Update File: src/a.rs\n@@\n-x\n+y\n*** Add File: src/b.rs\n+new\n*** Delete File: src/c.rs\n*** Update File: src/a.rs\n*** Move to: src/d.rs\n*** End Patch\n";
    assert_eq!(patch_paths(patch), vec!["src/a.rs", "src/b.rs", "src/d.rs"]);
    assert!(patch_paths("ls -la").is_empty());
}

#[test]
fn project_relative_strips_the_root() {
    assert_eq!(project_relative("/p/src/a.rs", Some("/p")), "src/a.rs");
    assert_eq!(project_relative("/q/a.rs", Some("/p")), "/q/a.rs");
    assert_eq!(project_relative("a.rs", None), "a.rs");
}

#[cfg(unix)]
#[test]
fn user_config_dir_follows_xdg() {
    let expected = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => dirs::home_dir().unwrap().join(".config"),
    };
    assert_eq!(user_config_dir(), Some(expected));
}

/// A global hook takes the ids from the project it runs in; with none
/// to find (and no backend) the reminder goes without them.
#[tokio::test]
async fn global_hook_env_resolves_lazily() {
    let project = tempfile::tempdir().unwrap();
    let config = tempfile::tempdir().unwrap();
    std::fs::write(
        project.path().join(".post-cortex.json"),
        r#"{"session_id":"S9","workspace_id":"W9"}"#,
    )
    .unwrap();
    let env = HookEnv::for_project(
        "http://127.0.0.1:9",
        reqwest::Client::new(),
        "codex",
        ProjectSource {
            dir: project.path().to_path_buf(),
            config_dir: config.path().to_path_buf(),
            workspace: None,
            identity: None,
        },
        false,
    );
    assert_eq!(
        env.ids().await.map(|ids| ids.session_id.as_str()),
        Some("S9")
    );
    assert!(
        env.rules_reminder()
            .await
            .starts_with("[PCX] Session: S9 | Workspace: W9 |")
    );

    // Capture off in config.json: turns are dropped, the daemon (port 9,
    // unreachable) is never called.
    assert!(
        env.capture(vec![Turn {
            turn_id: "t".into(),
            role: "user",
            text: "hi".into(),
            source_session: None,
            tools: vec![],
            files: vec![],
        }])
        .await
        .is_ok()
    );

    let lost = tempfile::tempdir().unwrap();
    let env = HookEnv::for_project(
        "http://127.0.0.1:9",
        reqwest::Client::new(),
        "codex",
        ProjectSource {
            dir: lost.path().to_path_buf(),
            config_dir: config.path().to_path_buf(),
            workspace: None,
            identity: None,
        },
        true,
    );
    assert!(env.ids().await.is_none());
    assert!(env.rules_reminder().await.starts_with("[PCX] Rules:"));
    assert_eq!(env.cards_text().await, None);
    assert!(
        env.capture(vec![Turn {
            turn_id: "t".into(),
            role: "user",
            text: "hi".into(),
            source_session: None,
            tools: vec![],
            files: vec![],
        }])
        .await
        .is_err()
    );
}

/// Cached ids the backend refuses (HTTP 404 for the workspace) are dropped,
/// so the next hook resolves the project again; ids pinned in the
/// repository are never touched.
#[tokio::test]
async fn refused_cached_ids_are_dropped() {
    let app = axum::Router::new().route(
        "/api/events",
        axum::routing::post(|| async { (axum::http::StatusCode::NOT_FOUND, "no such workspace") }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await });

    let project = tempfile::tempdir().unwrap();
    let config = tempfile::tempdir().unwrap();
    let key = crate::project::project(project.path()).key;
    let identity = crate::project::identity(Some("tok"));
    let stale = crate::project::Ids {
        session_id: "S-gone".into(),
        workspace_id: "W-gone".into(),
    };
    crate::project::cache_ids(config.path(), &key, &url, identity.as_deref(), &stale);
    let env = |identity: Option<String>| {
        HookEnv::for_project(
            &url,
            reqwest::Client::new(),
            "codex",
            ProjectSource {
                dir: project.path().to_path_buf(),
                config_dir: config.path().to_path_buf(),
                workspace: None,
                identity,
            },
            false,
        )
    };
    let hook = env(identity.clone());
    assert_eq!(hook.ids().await, Some(&stale));
    assert_eq!(hook.cards_text().await, None);
    assert_eq!(
        crate::project::cached_ids(config.path(), &key, &url, identity.as_deref()),
        None
    );

    std::fs::write(
        project.path().join(crate::project::PINNED_FILE),
        r#"{"session_id":"S-gone","workspace_id":"W-gone"}"#,
    )
    .unwrap();
    let hook = env(identity);
    assert_eq!(hook.cards_text().await, None);
    assert!(project.path().join(crate::project::PINNED_FILE).exists());
}

#[test]
fn payload_dir_from_cwd_or_workspace_roots() {
    assert_eq!(
        payload_dir(&json!({"cwd": "/a"})),
        Some(PathBuf::from("/a"))
    );
    assert_eq!(
        payload_dir(&json!({"workspace_roots": ["/b", "/c"]})),
        Some(PathBuf::from("/b"))
    );
    assert_eq!(payload_dir(&json!({"cwd": " "})), None);
    assert_eq!(payload_dir(&Value::Null), None);
}

#[test]
fn global_instructions_name_no_ids() {
    let block = global_agents_md_block();
    assert!(
        !block.contains("<SESSION_ID>") && !block.contains("|---|---|"),
        "{block}"
    );
    assert!(block.contains("given at session start"), "{block}");
    assert!(block.contains("with that session ID"), "{block}");
    assert!(!block.contains("\n\n\n"), "{block}");
    let dir = tempfile::tempdir().unwrap();
    let global = SetupContext {
        global: true,
        ..ctx(dir.path())
    };
    assert_eq!(
        global.hook_command(Client::Gemini, "prompt"),
        "pcx hook gemini prompt --profile self --url http://127.0.0.1:3737"
    );
    assert_eq!(
        report_path(&dir.path().join(".gemini/x"), &global),
        "~/.gemini/x"
    );
}

#[test]
fn turn_ids_are_stable_and_distinct() {
    assert_eq!(turn_id("c", "user", "hi"), turn_id("c", "user", "hi"));
    assert_ne!(turn_id("c", "user", "hi"), turn_id("c", "assistant", "hi"));
    assert_eq!(urlencode("src/a b.rs"), "src/a%20b.rs");
}
