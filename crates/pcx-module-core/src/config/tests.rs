use pbjson_types::Any;
use serde_json::{Value, json};

use crate::TurnGrant;
use crate::{Manifest, ModuleError, ModuleErrorKind};

use super::*;

mod patterns;

fn schema(value: Value) -> Result<FormSchema, SchemaError> {
    match value {
        Value::Object(schema) => FormSchema::parse(&schema),
        _ => panic!("a test schema is an object"),
    }
}

/// A schema with each kind of setting.
fn slack() -> FormSchema {
    schema(json!({
        "type": "object",
        "title": "Slack",
        "properties": {
            "token": { "type": "string", "writeOnly": true, "description": "Bot token." },
            "mode": { "type": "string", "enum": ["quiet", "loud"], "default": "quiet" },
            "retries": { "type": "integer", "default": 3 },
            "ratio": { "type": "number" },
            "enabled": { "type": "boolean" },
            "channels": { "type": "array", "items": { "type": "string" } },
            "admin": {
                "type": "object",
                "properties": { "key": { "type": "string", "writeOnly": true } }
            }
        },
        "required": ["token"]
    }))
    .unwrap()
}

#[test]
fn a_schema_of_the_subset_is_read() {
    let s = slack();
    let fields: Vec<&str> = s.fields().collect();
    assert_eq!(
        fields,
        [
            "admin", "channels", "enabled", "mode", "ratio", "retries", "token"
        ]
    );
}

#[test]
fn a_schema_outside_the_subset_is_refused_where_it_is() {
    let cases = [
        (json!({ "type": "object", "pattern": "x" }), "/", "pattern"),
        (json!({ "type": "string" }), "/", "is an object"),
        (
            json!({ "type": "object", "properties": { "a": { "type": "string", "format": "uri" } } }),
            "properties.a",
            "format uri",
        ),
        (
            json!({ "type": "object", "properties": { "a": { "type": "integer", "format": "workspace" } } }),
            "properties.a",
            "format is for a string",
        ),
        (
            json!({ "type": "object", "properties": { "a": { "description": "no type" } } }),
            "properties.a",
            "needs a type",
        ),
        (
            json!({ "type": "object", "properties": { "a": { "type": "null" } } }),
            "properties.a",
            "null",
        ),
        (
            json!({ "type": "object", "properties": { "a": { "type": ["string", "null"] } } }),
            "properties.a",
            "one type name",
        ),
        (
            json!({ "type": "object", "properties": { "a": { "type": "object", "writeOnly": true } } }),
            "properties.a",
            "single value",
        ),
        (
            json!({ "type": "object", "properties": { "a": { "type": "array", "items": { "type": "string", "writeOnly": true } } } }),
            "properties.a.items",
            "inside an array",
        ),
        (
            json!({ "type": "object", "properties": { "a": { "type": "array" } } }),
            "properties.a",
            "needs items",
        ),
        (
            json!({ "type": "object", "properties": { "a": { "type": "string", "items": { "type": "string" } } } }),
            "properties.a",
            "belongs to an array",
        ),
        (
            json!({ "type": "object", "properties": { "a": { "type": "boolean", "enum": [true] } } }),
            "properties.a",
            "enum is for",
        ),
        (
            json!({ "type": "object", "properties": { "a": { "type": "string", "enum": ["x", 1] } } }),
            "properties.a",
            "enum",
        ),
        (
            json!({ "type": "object", "properties": { "a": { "type": "integer", "default": 1.5 } } }),
            "properties.a",
            "default",
        ),
        (
            json!({ "type": "object", "properties": { "a.b": { "type": "string" } } }),
            "/",
            "not allowed",
        ),
        (
            json!({ "type": "object", "properties": {}, "required": ["a"] }),
            "/",
            "not a setting",
        ),
    ];
    for (value, at, reason) in cases {
        let e = schema(value.clone()).expect_err(&value.to_string());
        assert_eq!(e.at, at, "{value}");
        assert!(e.reason.contains(reason), "{value}: {}", e.reason);
    }
}

/// A manifest whose configuration is `config_type` of `descriptors`.
fn configured(descriptors: &[u8], config_type: &str) -> Manifest {
    Manifest {
        descriptors: descriptors.to_vec(),
        config_type: config_type.to_string(),
        ..Manifest::default()
    }
}

/// Telegram's settings: a required secret token, an API URL.
fn telegram() -> ConfigType {
    let manifest = configured(
        post_cortex_proto::TELEGRAM_DESCRIPTOR_SET,
        "pcx.telegram.v1.TelegramConfig",
    );
    ConfigType::of(&manifest).unwrap().unwrap()
}

/// The chief's settings: a required workspace, a grant.
fn chief() -> ConfigType {
    let manifest = configured(
        post_cortex_proto::CHIEF_DESCRIPTOR_SET,
        "pcx.chief.v1.ChiefConfig",
    );
    ConfigType::of(&manifest).unwrap().unwrap()
}

const WORKSPACE: &str = "6f1c2a3e-0000-4000-8000-000000000001";

#[test]
fn a_manifest_without_a_config_type_takes_none() {
    assert!(ConfigType::of(&Manifest::default()).unwrap().is_none());
    let missing = configured(post_cortex_proto::CHIEF_DESCRIPTOR_SET, "pcx.chief.v1.Nope");
    assert!(
        ConfigType::of(&missing)
            .unwrap_err()
            .reason
            .contains("Nope")
    );
}

#[test]
fn a_message_that_is_no_setting_is_refused() {
    // A TalkRecord has a Timestamp: a well-known type is no setting.
    let talk = configured(
        post_cortex_proto::CHIEF_DESCRIPTOR_SET,
        "pcx.chief.v1.TalkRecord",
    );
    let e = ConfigType::of(&talk).unwrap_err();
    assert!(e.reason.contains("google.protobuf"), "{e}");
}

#[test]
fn a_configuration_is_read_from_its_json_and_checked() {
    let telegram = telegram();
    let config = telegram
        .from_json(&json!({ "bot_token": "k", "api_url": "https://x" }))
        .unwrap();
    assert!(config.type_url.ends_with("pcx.telegram.v1.TelegramConfig"));
    assert_eq!(
        telegram.to_json(&config).unwrap(),
        json!({ "bot_token": "k", "api_url": "https://x" })
    );
    telegram.check(&config).unwrap();
    let missing = telegram
        .from_json(&json!({ "api_url": "https://x" }))
        .unwrap();
    assert_eq!(telegram.check(&missing).unwrap_err().field, "bot_token");
    assert!(telegram.from_json(&json!({ "nope": 1 })).is_err());
    let empty = telegram.from_json(&Value::Null).unwrap();
    assert_eq!(telegram.check(&empty).unwrap_err().field, "bot_token");
}

#[test]
fn a_workspace_setting_takes_a_workspace_id() {
    let chief = chief();
    let read = |config: Value| chief.check(&chief.from_json(&config).unwrap());
    assert!(read(json!({ "workspace_id": WORKSPACE })).is_ok());
    let e = read(json!({ "workspace_id": "w" })).unwrap_err();
    assert_eq!(e.field, "workspace_id");
    assert!(e.reason.contains("workspace id"));
}

#[test]
fn a_board_setting_takes_a_board_id_or_none() {
    let manifest = configured(
        post_cortex_proto::WORKFLOW_DESCRIPTOR_SET,
        "pcx.workflow.v1.WorkflowConfig",
    );
    let workflow = ConfigType::of(&manifest).unwrap().unwrap();
    let read = |config: Value| workflow.check(&workflow.from_json(&config).unwrap());
    assert!(read(json!({ "workspace_id": WORKSPACE, "board": WORKSPACE })).is_ok());
    assert!(read(json!({ "workspace_id": WORKSPACE })).is_ok());
    let e = read(json!({ "workspace_id": WORKSPACE, "board": "pcx-cloud" })).unwrap_err();
    assert_eq!(e.field, "board");
    assert!(e.reason.contains("board id"));
}

#[test]
fn a_refused_configuration_is_an_invalid_argument_naming_the_field() {
    let telegram = telegram();
    let e: ModuleError = telegram
        .check(&telegram.from_json(&json!({})).unwrap())
        .unwrap_err()
        .into();
    assert_eq!(e.kind, ModuleErrorKind::InvalidArgument);
    assert_eq!(e.field.as_deref(), Some("bot_token"));
}

#[test]
fn secrets_are_not_shown_but_said_to_be_set() {
    let telegram = telegram();
    let config = telegram
        .from_json(&json!({ "bot_token": "k", "api_url": "e" }))
        .unwrap();
    let (shown, set) = telegram.redact(&config).unwrap();
    assert_eq!(telegram.to_json(&shown).unwrap(), json!({ "api_url": "e" }));
    assert_eq!(set, ["bot_token"]);
}

#[test]
fn a_change_without_a_secret_keeps_it_and_clear_removes_it() {
    let telegram = telegram();
    let current = telegram.from_json(&json!({ "bot_token": "k" })).unwrap();
    let change = telegram
        .from_json(&json!({ "bot_token": "k2", "api_url": "e" }))
        .unwrap();
    let (unset, _) = telegram.redact(&change).unwrap();
    let kept = telegram.keep_secrets(&unset, &[], &current).unwrap();
    assert_eq!(
        telegram.to_json(&kept).unwrap(),
        json!({ "bot_token": "k", "api_url": "e" })
    );
    let replaced = telegram.keep_secrets(&change, &[], &current).unwrap();
    assert_eq!(telegram.to_json(&replaced).unwrap()["bot_token"], "k2");
    let cleared = telegram
        .keep_secrets(&unset, &["bot_token".to_string()], &current)
        .unwrap();
    assert_eq!(
        telegram.to_json(&cleared).unwrap(),
        json!({ "api_url": "e" })
    );
    let e = telegram
        .keep_secrets(&unset, &["api_url".to_string()], &current)
        .unwrap_err();
    assert_eq!(e.field, "api_url");
}

#[test]
fn the_grant_of_a_modules_turns_is_its_grant_field() {
    let chief = chief();
    let config = chief
        .from_json(&json!({
            "workspace_id": WORKSPACE,
            "grant": { "person_tools": ["team_create"], "withheld": ["comment"] },
        }))
        .unwrap();
    let grant = chief.grant(&config).unwrap();
    assert_eq!(grant.person_tools, ["team_create"]);
    assert_eq!(grant.withheld, ["comment"]);
    assert_eq!(
        telegram().grant(&Any::default()).unwrap(),
        TurnGrant::default()
    );
}

#[test]
fn the_persons_a_module_acts_for_are_its_person_grant_field() {
    let manifest = configured(
        post_cortex_proto::TELEGRAM_DESCRIPTOR_SET,
        "pcx.telegram.v1.TelegramConfig",
    );
    let telegram = ConfigType::of(&manifest).unwrap().unwrap();
    let config = telegram
        .from_json(&json!({
            "bot_token": "T",
            "persons": { "persons": ["julius", "ana"] },
        }))
        .unwrap();
    assert_eq!(telegram.persons(&config).unwrap(), ["julius", "ana"]);
    let none = telegram.from_json(&json!({ "bot_token": "T" })).unwrap();
    assert!(telegram.persons(&none).unwrap().is_empty());
    assert!(
        chief().persons(&Any::default()).unwrap().is_empty(),
        "no such field"
    );
}

/// The http module's settings: connections, each named by its key, with
/// a secret.
fn http() -> ConfigType {
    let manifest = configured(
        post_cortex_proto::HTTP_DESCRIPTOR_SET,
        "pcx.http.v1.HttpConfig",
    );
    ConfigType::of(&manifest).unwrap().unwrap()
}

fn connection(name: &str, secret: Option<&str>) -> Value {
    let mut c = json!({ "name": name, "base_url": format!("https://{name}.example") });
    if let Some(secret) = secret {
        c["secret"] = json!(secret);
    }
    c
}

#[test]
fn a_secret_in_a_keyed_list_is_named_by_its_element() {
    let http = http();
    let config = http
        .from_json(&json!({
            "workspace_id": WORKSPACE,
            "connections": [connection("github", Some("g")), connection("slack", None)],
        }))
        .unwrap();
    let (shown, set) = http.redact(&config).unwrap();
    assert_eq!(set, ["connections[github].secret"]);
    assert!(
        http.to_json(&shown).unwrap()["connections"][0]
            .get("secret")
            .is_none()
    );
    assert_eq!(
        http.secrets(&config).unwrap(),
        [("connections[github].secret".to_string(), "g".to_string())]
    );
    let opened = http
        .with_secrets(&shown, &http.secrets(&config).unwrap())
        .unwrap();
    assert_eq!(
        http.to_json(&opened).unwrap(),
        http.to_json(&config).unwrap()
    );
    let e = http
        .with_secrets(
            &shown,
            &[("connections[nope].secret".to_string(), "x".to_string())],
        )
        .unwrap_err();
    assert_eq!(e.field, "connections[nope].secret");
}

#[test]
fn a_change_keeps_an_elements_secret_by_its_key() {
    let http = http();
    let current = http
        .from_json(&json!({
            "workspace_id": WORKSPACE,
            "connections": [connection("github", Some("g")), connection("slack", Some("s"))],
        }))
        .unwrap();
    // Reordered, slack's secret replaced, github's left unset, a new one.
    let change = http
        .from_json(&json!({
            "workspace_id": WORKSPACE,
            "connections": [
                connection("slack", Some("s2")),
                connection("stripe", None),
                connection("github", None),
            ],
        }))
        .unwrap();
    let kept = http.keep_secrets(&change, &[], &current).unwrap();
    assert_eq!(
        http.secrets(&kept).unwrap(),
        [
            ("connections[slack].secret".to_string(), "s2".to_string()),
            ("connections[github].secret".to_string(), "g".to_string()),
        ]
    );
    // An element left out takes its secret; a renamed one is a new one.
    let renamed = http
        .from_json(&json!({
            "workspace_id": WORKSPACE,
            "connections": [connection("gh", None)],
        }))
        .unwrap();
    let kept = http.keep_secrets(&renamed, &[], &current).unwrap();
    assert!(http.secrets(&kept).unwrap().is_empty());
    let cleared = http
        .keep_secrets(
            &change,
            &["connections[github].secret".to_string()],
            &current,
        )
        .unwrap();
    assert_eq!(
        http.secrets(&cleared).unwrap(),
        [("connections[slack].secret".to_string(), "s2".to_string())]
    );
    let e = http
        .keep_secrets(&change, &["connections.secret".to_string()], &current)
        .unwrap_err();
    assert_eq!(e.field, "connections.secret");
}

#[test]
fn a_lists_keys_name_its_elements_once() {
    let http = http();
    let config = |names: &[&str]| {
        http.from_json(&json!({
            "workspace_id": WORKSPACE,
            "connections": names.iter().map(|n| connection(n, None)).collect::<Vec<_>>(),
        }))
        .unwrap()
    };
    assert!(http.check(&config(&["github", "slack-2"])).is_ok());
    let e = http.check(&config(&["github", "github"])).unwrap_err();
    assert_eq!(e.field, "connections[1].name");
    let e = http.check(&config(&["GitHub"])).unwrap_err();
    assert_eq!(e.field, "connections[0].name");
    let e = http.check(&config(&[""])).unwrap_err();
    assert_eq!(e.field, "connections[0].name");
}
