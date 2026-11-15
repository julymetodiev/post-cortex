// Copyright (c) 2026 Julius ML
// MIT License

use super::*;
use crate::profiles::store::loose_mode;
use std::path::Path;

const LOCAL: &str = "http://127.0.0.1:3737";

fn store_env(dir: &Path, daemon_token: Option<&str>, env: Env) -> Store {
    Store::at(
        dir.to_path_buf(),
        LOCAL.into(),
        daemon_token.map(String::from),
        env,
    )
}

fn store(dir: &Path, daemon_token: Option<&str>, token: Option<&str>) -> Store {
    let env = Env {
        token: token.map(String::from),
        ..Env::default()
    };
    store_env(dir, daemon_token, env)
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn defaults_without_a_file() {
    let dir = tempfile::tempdir().unwrap();
    let s = store(dir.path(), None, None);
    assert_eq!(s.active().unwrap(), CLOUD);
    let active = s.resolve(None).unwrap();
    assert_eq!(active.name, CLOUD);
    assert_eq!(active.url, CLOUD_URL);
    assert_eq!(active.token, None);
    assert!(active.remote());
    assert_eq!(active.mcp_url(), "https://api.pcx.is/mcp");
    let local = s.resolve(Some(SELF_HOSTED)).unwrap();
    assert_eq!(local.url, LOCAL);
    assert!(!local.remote());
    assert_eq!(
        s.list().unwrap(),
        vec![
            (CLOUD.to_string(), CLOUD_URL.to_string()),
            (SELF_HOSTED.to_string(), LOCAL.to_string())
        ]
    );
    assert!(s.resolve(Some("nope")).is_err());
    assert!(!dir.path().join(CONFIG_FILE).exists());
}

#[test]
fn add_use_remove_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let s = store(dir.path(), None, None);
    s.add("work", "https://pcx.corp.example/").unwrap();
    s.use_profile("work").unwrap();
    let v = read_json(&dir.path().join(CONFIG_FILE));
    assert_eq!(v["active"], "work");
    assert_eq!(v["profiles"]["work"]["url"], "https://pcx.corp.example");
    let work = s.resolve(None).unwrap();
    assert_eq!(work.url, "https://pcx.corp.example");
    assert!(work.remote());

    // A built-in's URL can be overridden and the override removed.
    s.add(SELF_HOSTED, "http://ml.example.ts.net:3737").unwrap();
    assert_eq!(
        s.resolve(Some(SELF_HOSTED)).unwrap().url,
        "http://ml.example.ts.net:3737"
    );
    s.remove(SELF_HOSTED).unwrap();
    assert_eq!(s.resolve(Some(SELF_HOSTED)).unwrap().url, LOCAL);

    s.store_token("work", "w-token").unwrap();
    s.remove("work").unwrap();
    assert_eq!(s.active().unwrap(), CLOUD);
    assert_eq!(s.stored_token("work").unwrap(), None);
    assert!(s.remove("work").is_err());
    assert!(s.use_profile("work").is_err());
    assert!(s.add("bad name", "https://x.example").is_err());
    assert!(s.add("x", "ftp://x").is_err());
}

/// Other tools write `hosts` and keys of their own; a save keeps them.
#[test]
fn unknown_keys_survive_a_save() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(CONFIG_FILE),
        r#"{"active":"self","profiles":{"self":{"url":"http://a:1","note":"x"}},"hosts":{"opencode":{"profile":"cloud"}},"future":1}"#,
    )
    .unwrap();
    let s = store(dir.path(), None, None);
    assert_eq!(s.active().unwrap(), SELF_HOSTED);
    s.add(SELF_HOSTED, "http://b:2").unwrap();
    s.use_profile(CLOUD).unwrap();
    let v = read_json(&dir.path().join(CONFIG_FILE));
    assert_eq!(v["active"], "cloud");
    assert_eq!(
        v["profiles"]["self"],
        serde_json::json!({"url":"http://b:2","note":"x"})
    );
    assert_eq!(v["hosts"]["opencode"]["profile"], "cloud");
    assert_eq!(v["future"], 1);
}

#[test]
fn host_settings_default_and_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let s = store(dir.path(), None, None);
    let defaults = HostSettings {
        enabled: true,
        capture: false,
        workspace: None,
    };
    assert_eq!(s.host("claude-code").unwrap(), defaults);
    s.add("work", "https://pcx.corp.example").unwrap();
    s.set_host("claude-code", "capture", Value::Bool(true))
        .unwrap();
    s.set_host("codex", "workspace", "Team".into()).unwrap();
    s.set_host("codex", "enabled", Value::Bool(false)).unwrap();
    assert!(s.host("claude-code").unwrap().capture);
    assert_eq!(
        s.host("codex").unwrap(),
        HostSettings {
            enabled: false,
            capture: false,
            workspace: Some("Team".into()),
        }
    );
    let v = read_json(&dir.path().join(CONFIG_FILE));
    assert_eq!(v["profiles"]["work"]["url"], "https://pcx.corp.example");
    assert_eq!(
        v["hosts"]["claude-code"],
        serde_json::json!({"capture": true})
    );
}

#[test]
fn environment_overrides_profile_and_url() {
    let dir = tempfile::tempdir().unwrap();
    let s = store(dir.path(), None, None);
    s.add("work", "https://pcx.corp.example").unwrap();
    let env = Env {
        profile: Some("work".into()),
        url: Some("https://override.example/".into()),
        token: None,
    };
    let s = store_env(dir.path(), None, env);
    assert_eq!(s.active().unwrap(), "work");
    let p = s.resolve(None).unwrap();
    assert_eq!(p.name, "work");
    assert_eq!(p.url, "https://override.example");
    // PC_URL also makes an unknown profile usable.
    assert_eq!(
        s.resolve(Some("adhoc")).unwrap().url,
        "https://override.example"
    );
    let blank = store_env(
        dir.path(),
        None,
        Env {
            profile: Some(" ".into()),
            ..Env::default()
        },
    );
    assert_eq!(blank.active().unwrap(), CLOUD);
}

#[test]
fn list_shows_the_url_in_use_and_who_picked_the_profile() {
    let dir = tempfile::tempdir().unwrap();
    let plain = store(dir.path(), None, None);
    plain.store_token(CLOUD, "pcx_secret").unwrap();
    assert_eq!(
        plain.list_lines().unwrap(),
        vec![
            format!("* cloud        {CLOUD_URL}  (logged in)"),
            format!("  self         {LOCAL}"),
        ]
    );

    let env = Env {
        url: Some("http://127.0.0.1:8080/".into()),
        ..Env::default()
    };
    assert_eq!(
        store_env(dir.path(), None, env).list_lines().unwrap(),
        vec![
            format!(
                "* cloud        http://127.0.0.1:8080 (PC_URL; profile: {CLOUD_URL})  (logged in)"
            ),
            format!("  self         http://127.0.0.1:8080 (PC_URL; profile: {LOCAL})"),
        ]
    );

    let env = Env {
        profile: Some(SELF_HOSTED.into()),
        ..Env::default()
    };
    assert_eq!(
        store_env(dir.path(), None, env).list_lines().unwrap(),
        vec![
            format!("  cloud        {CLOUD_URL}  (logged in)"),
            format!("* self         {LOCAL}  (PC_PROFILE)"),
        ]
    );

    // An unknown profile picked by PC_PROFILE, usable through PC_URL.
    let env = Env {
        profile: Some("adhoc".into()),
        url: Some("http://10.0.0.2:3737".into()),
        ..Env::default()
    };
    let lines = store_env(dir.path(), None, env).list_lines().unwrap();
    assert_eq!(
        lines.last().unwrap(),
        "* adhoc        http://10.0.0.2:3737 (PC_URL; no such profile)  (PC_PROFILE)"
    );
}

#[test]
fn token_resolution_order() {
    let dir = tempfile::tempdir().unwrap();
    // Nothing anywhere, then daemon.toml for `self` only.
    let s = store(dir.path(), Some("daemon-token"), None);
    assert_eq!(s.token(CLOUD).unwrap(), None);
    assert_eq!(
        s.token(SELF_HOSTED).unwrap().as_deref(),
        Some("daemon-token")
    );
    // Credentials beat daemon.toml.
    s.store_token(SELF_HOSTED, "stored-self").unwrap();
    s.store_token(CLOUD, " stored-cloud \n").unwrap();
    assert_eq!(
        s.token(SELF_HOSTED).unwrap().as_deref(),
        Some("stored-self")
    );
    assert_eq!(s.token(CLOUD).unwrap().as_deref(), Some("stored-cloud"));
    // PC_AUTH_TOKEN beats everything; an empty one counts as unset.
    let env = store(dir.path(), Some("daemon-token"), Some("env-token"));
    assert_eq!(env.token(CLOUD).unwrap().as_deref(), Some("env-token"));
    assert_eq!(
        env.token(SELF_HOSTED).unwrap().as_deref(),
        Some("env-token")
    );
    let empty = store(dir.path(), None, Some("  "));
    assert_eq!(empty.token(CLOUD).unwrap().as_deref(), Some("stored-cloud"));
    // Logout.
    assert!(s.forget_token(CLOUD).unwrap());
    assert!(!s.forget_token(CLOUD).unwrap());
    assert_eq!(s.resolve(Some(CLOUD)).unwrap().token, None);
}

#[cfg(unix)]
#[test]
fn credentials_are_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let s = store(dir.path(), None, None);
    s.store_token(CLOUD, "t").unwrap();
    let path = dir.path().join(CREDENTIALS_FILE);
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&path), 0o600);
    assert_eq!(loose_mode(&path), None);
    // A file someone loosened is reported, and the next write tightens it.
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(loose_mode(&path), Some(0o644));
    s.store_token("other", "u").unwrap();
    assert_eq!(mode(&path), 0o600);
    let v = read_json(&path);
    assert_eq!(v["cloud"]["token"], "t");
    assert_eq!(v["other"]["token"], "u");
}
