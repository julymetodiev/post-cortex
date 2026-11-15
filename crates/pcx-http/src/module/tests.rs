// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The module's settings as the host reads them: its message is a form
//! the host accepts, each connection's secret is named by the connection
//! and never shown, and what the host stores reads into the settings.

use pcx_module_api::{ConfigType, Module};
use prost::Message;
use serde_json::json;

use super::*;

#[test]
fn the_host_reads_its_settings_and_keeps_each_connections_secret() {
    let module = HttpModule::new(true);
    let form = ConfigType::of(module.manifest()).unwrap().unwrap();
    let config = form
        .from_json(&json!({
            "workspace_id": "6f1d2c3b-4a5e-4f60-8a7b-9c0d1e2f3a4b",
            "connections": [
                {"name": "github", "base_url": "https://api.github.com", "auth": "AUTH_BEARER", "secret": "ghp_1"},
                {"name": "slack", "base_url": "https://slack.com/api", "auth": "AUTH_BEARER", "secret": "xoxb_2"},
            ],
        }))
        .unwrap();
    form.check(&config).unwrap();
    let (shown, set) = form.redact(&config).unwrap();
    assert_eq!(
        set,
        ["connections[github].secret", "connections[slack].secret"]
    );
    let shown = form.to_json(&shown).unwrap().to_string();
    assert!(!shown.contains("ghp_1") && !shown.contains("xoxb_2"));

    let read = HttpConfig::decode(&config.value[..]).unwrap();
    let settings = domain::settings(read.clone(), true).unwrap();
    assert_eq!(settings.connections.len(), 2);
    let mut private = read;
    private.private_networks = true;
    assert_eq!(
        domain::settings(private, true).unwrap_err().field,
        "private_networks"
    );
}
