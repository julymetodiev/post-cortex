// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The module's settings as the host reads them: its message is a form
//! the host accepts, its secrets are said to be set and never shown, and
//! what the host stores reads into the settings the module sends with.

use pcx_module_api::{ConfigType, Module};
use prost::Message;
use serde_json::json;

use super::*;
use crate::domain::Route;

#[test]
fn the_host_reads_its_settings_and_keeps_its_secrets() {
    let module = MailModule::new();
    let form = ConfigType::of(module.manifest()).unwrap().unwrap();
    let config = form
        .from_json(&json!({
            "workspace_id": "6f1d2c3b-4a5e-4f60-8a7b-9c0d1e2f3a4b",
            "from": {"address": "reports@example.com", "name": "Reports"},
            "transport": "TRANSPORT_SMTP",
            "smtp": {"host": "smtp.example.com", "user": "reports", "password": "secret"},
            "allow": ["@example.com"],
        }))
        .unwrap();
    form.check(&config).unwrap();
    let (shown, set) = form.redact(&config).unwrap();
    assert_eq!(set, vec!["smtp.password".to_string()]);
    assert!(!form.to_json(&shown).unwrap().to_string().contains("secret"));

    let settings = domain::settings(MailConfig::decode(&config.value[..]).unwrap()).unwrap();
    assert_eq!(
        settings.route,
        Route::Smtp {
            host: "smtp.example.com".into(),
            port: 587,
            secured: domain::Secured::StartTls,
            credentials: Some(("reports".into(), "secret".into())),
        }
    );
    assert_eq!(settings.allow, vec!["@example.com".to_string()]);
}
