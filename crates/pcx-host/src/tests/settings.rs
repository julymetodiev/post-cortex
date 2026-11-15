// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Module settings: a configuration checked against its message before the
//! module starts, listed without its secrets, and changed through the
//! host's settings port.

use super::*;

use serde_json::json;

/// The message of the configuration: Telegram's, a secret token and an
/// API URL.
const CONFIG: &str = "pcx.telegram.v1.TelegramConfig";

/// A tool module whose configuration has a token (a required secret) and
/// an API URL.
fn configurable() -> Fake {
    let mut module = tool(Act::Answer);
    module.manifest.descriptors = post_cortex_proto::TELEGRAM_DESCRIPTOR_SET.to_vec();
    module.manifest.config_type = CONFIG.to_string();
    module
}

/// `value` as a configuration of [`configurable`].
fn any(value: Value) -> pbjson_types::Any {
    let manifest = configurable().manifest;
    pcx_module_api::ConfigType::of(&manifest)
        .unwrap()
        .unwrap()
        .from_json(&value)
        .unwrap()
}

fn config(value: Value) -> HashMap<String, Value> {
    HashMap::from([("tool".to_string(), value)])
}

/// A settings port that records what it is given.
#[derive(Default)]
struct Recorded {
    applied: parking_lot::Mutex<Vec<(String, Value)>>,
    refuse: bool,
}

#[async_trait]
impl Settings for Recorded {
    async fn apply(&self, id: &str, config: Value) -> Result<(), SettingsError> {
        if self.refuse {
            return Err(SettingsError::NotConfigurable {
                module: id.to_string(),
            });
        }
        self.applied.lock().push((id.to_string(), config));
        Ok(())
    }

    async fn grant(&self, id: &str, _grants: ModuleGrants) -> Result<(), SettingsError> {
        Err(SettingsError::Unknown {
            module: id.to_string(),
        })
    }

    fn available(&self) -> Vec<String> {
        Vec::new()
    }
}

async fn running() -> Arc<Host> {
    let host = Host::start(
        vec![Arc::new(configurable())],
        &config(json!({ "bot_token": "secret", "api_url": "https://a" })),
        &HashMap::new(),
    )
    .await
    .unwrap();
    Arc::new(host)
}

#[tokio::test]
async fn a_configuration_outside_the_schema_is_refused_before_start() {
    let refused = Host::start(
        vec![Arc::new(configurable())],
        &config(json!({ "api_url": "https://b" })),
        &HashMap::new(),
    )
    .await;
    match refused {
        Err(HostError::InvalidConfig { module, error }) => {
            assert_eq!(module, "tool");
            assert_eq!(error.field, "bot_token");
        }
        other => panic!("expected InvalidConfig, got {:?}", other.err()),
    }
}

#[tokio::test]
async fn a_message_that_is_no_settings_refuses_the_manifest() {
    let mut module = configurable();
    module.manifest.config_type = "pcx.telegram.v1.Nope".to_string();
    let refused = Host::start(vec![Arc::new(module)], &HashMap::new(), &HashMap::new()).await;
    assert!(matches!(refused, Err(HostError::InvalidManifest { .. })));
}

#[tokio::test]
async fn modules_are_listed_without_their_secrets() {
    let host = running().await;
    let listed = host.modules();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].manifest.id, "tool");
    let shown = pcx_module_api::ConfigType::of(&configurable().manifest)
        .unwrap()
        .unwrap()
        .to_json(&listed[0].config)
        .unwrap();
    assert_eq!(shown, json!({ "api_url": "https://a" }));
    assert_eq!(listed[0].secrets_set, ["bot_token"]);
    assert_eq!(listed[0].health.calls, 0);
}

#[tokio::test]
async fn a_change_keeps_its_secret_and_goes_to_the_port() {
    let host = running().await;
    let store = Arc::new(Recorded::default());
    let settings = ModuleSettings::new(Arc::clone(&host), Some(store.clone()));
    settings
        .configure("tool", &any(json!({ "api_url": "https://b" })), &[])
        .await
        .unwrap();
    assert_eq!(
        *store.applied.lock(),
        [(
            "tool".to_string(),
            json!({ "bot_token": "secret", "api_url": "https://b" })
        )]
    );
}

#[tokio::test]
async fn a_change_is_refused_by_kind() {
    let host = running().await;
    let store = Arc::new(Recorded::default());
    let settings = ModuleSettings::new(Arc::clone(&host), Some(store.clone()));
    let clear = |field: &str| vec![field.to_string()];

    let cleared = settings
        .configure("tool", &any(json!({})), &clear("bot_token"))
        .await
        .unwrap_err();
    assert_eq!(cleared.kind, ModuleErrorKind::InvalidArgument);
    assert_eq!(cleared.field.as_deref(), Some("bot_token"));
    let no_secret = settings
        .configure("tool", &any(json!({})), &clear("api_url"))
        .await
        .unwrap_err();
    assert_eq!(no_secret.field.as_deref(), Some("api_url"));
    let unknown = settings
        .configure("nope", &any(json!({})), &[])
        .await
        .unwrap_err();
    assert_eq!(unknown.kind, ModuleErrorKind::NotFound);
    assert!(store.applied.lock().is_empty());

    let fixed = ModuleSettings::new(Arc::clone(&host), None);
    let refused = fixed
        .configure("tool", &any(json!({})), &[])
        .await
        .unwrap_err();
    assert_eq!(refused.kind, ModuleErrorKind::FailedPrecondition);
    let elsewhere = ModuleSettings::new(
        host,
        Some(Arc::new(Recorded {
            refuse: true,
            ..Recorded::default()
        })),
    );
    let refused = elsewhere
        .configure("tool", &any(json!({})), &[])
        .await
        .unwrap_err();
    assert_eq!(refused.kind, ModuleErrorKind::FailedPrecondition);
}
