use std::os::unix::fs::PermissionsExt;

use serde_json::json;

use super::*;

const DAEMON_TOML: &str = r#"# The daemon of this machine.
port = 3737

# Greets people.
[[modules]]
id = "greet"
command = "/opt/pcx/greet"
sha256 = "ab12"
grants = []
config = { greeting = "Hello" } # set by hand

[[modules]]
id = "slack"
command = "/opt/pcx/slack"
sha256 = "cd34"

[modules.config]
channel = "general"
"#;

#[test]
fn only_the_modules_config_changes() {
    let text = with_config(DAEMON_TOML, "greet", &json!({ "greeting": "Hi" })).unwrap();
    assert!(text.starts_with("# The daemon of this machine.\nport = 3737\n\n# Greets people.\n"));
    assert!(
        text.contains(r#"config = { greeting = "Hi" } # set by hand"#),
        "{text}"
    );
    assert!(text.contains(r#"command = "/opt/pcx/greet""#));
    assert!(text.contains(r#"sha256 = "ab12""#));
    let parsed: crate::daemon::DaemonConfig = toml::from_str(&text).unwrap();
    assert_eq!(parsed.modules[0].config, Some(json!({ "greeting": "Hi" })));
    assert_eq!(
        parsed.modules[1].config,
        Some(json!({ "channel": "general" }))
    );
}

#[test]
fn a_config_table_stays_a_table() {
    let config = json!({ "channel": "ops", "retries": 3, "ratio": 0.5, "on": true, "tags": ["a"] });
    let text = with_config(DAEMON_TOML, "slack", &config).unwrap();
    assert!(
        text.contains("[modules.config]\nchannel = \"ops\""),
        "{text}"
    );
    assert!(
        text.find("[modules.config]") > text.find("id = \"slack\""),
        "{text}"
    );
    let parsed: crate::daemon::DaemonConfig = toml::from_str(&text).unwrap();
    assert_eq!(parsed.modules[1].config, Some(config));
    assert_eq!(
        parsed.modules[0].config,
        Some(json!({ "greeting": "Hello" }))
    );
}

#[test]
fn an_inline_modules_list_is_changed_in_place() {
    let inline = "modules = [{ id = \"greet\", command = \"/g\", sha256 = \"00\" }]\n";
    let text = with_config(inline, "greet", &json!({ "greeting": "Hi" })).unwrap();
    let parsed: crate::daemon::DaemonConfig = toml::from_str(&text).unwrap();
    assert_eq!(parsed.modules[0].config, Some(json!({ "greeting": "Hi" })));
}

#[test]
fn a_change_is_refused_by_kind() {
    assert!(matches!(
        with_config(DAEMON_TOML, "memory", &json!({})),
        Err(SettingsError::NotConfigurable { module }) if module == "memory"
    ));
    match with_config(DAEMON_TOML, "greet", &json!({ "a": { "b": [1, null] } })) {
        Err(SettingsError::Unrepresentable { reason }) => {
            assert!(reason.contains("a.b[1]"), "{reason}");
        }
        other => panic!("expected Unrepresentable, got {other:?}"),
    }
    assert!(matches!(
        with_config(DAEMON_TOML, "greet", &json!({ "n": u64::MAX })),
        Err(SettingsError::Unrepresentable { .. })
    ));
    assert!(matches!(
        with_config("port = ", "greet", &json!({})),
        Err(SettingsError::Store { .. })
    ));
}

#[test]
fn a_write_replaces_the_file_with_its_permissions() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("daemon.toml");
    std::fs::write(&path, "port = 1\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    write(&path, "port = 2\n").unwrap();
    assert_eq!(read(&path).unwrap(), "port = 2\n");
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    assert!(matches!(
        read(&dir.path().join("none.toml")),
        Err(SettingsError::Store { .. })
    ));
}
