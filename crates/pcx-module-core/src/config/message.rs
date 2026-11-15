// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A module's configuration as a message of its package (ARCHITECTURE
//! section 7, settings, W23): the message its manifest names
//! (`config_type`), read from its descriptors with each field's `setting`
//! option; the one check of a configuration, its secrets kept and cleared,
//! the grant of its turns, and its JSON form, which a configuration file
//! holds.

mod secrets;

use std::collections::HashSet;

use pbjson_types::Any;
use prost::Message;
use prost_reflect::{
    DynamicMessage, ExtensionDescriptor, FieldDescriptor, Kind, MessageDescriptor, ReflectMessage,
    SerializeOptions, Value as Field,
};
use serde_json::Value;

use crate::Manifest;
use crate::TurnGrant;
use crate::config::pattern::{declared, whole};
use crate::config::{ConfigError, SchemaError, descriptors};
use post_cortex_proto::module as wire;

use crate::name::is_slug;
use secrets::{Step, names};

/// The option of a settings field.
const SETTING: &str = "pcx.module.v1.setting";

/// The message of a turn's grant.
const GRANT: &str = "pcx.module.v1.TurnGrant";

/// The message of the persons a module acts for (W29).
const PERSONS: &str = "pcx.module.v1.PersonGrant";

/// The prefix of an `Any`'s type URL.
const TYPE_URL: &str = "type.googleapis.com/";

/// The message of a module's configuration.
#[derive(Debug, Clone)]
pub struct ConfigType {
    message: MessageDescriptor,
    /// The `setting` option, when its descriptors have it.
    setting: Option<ExtensionDescriptor>,
    /// The way to every secret, in field order.
    secrets: Vec<Vec<Step>>,
}

/// Where a field lies as to lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Within {
    /// In no list.
    Outside,
    /// In an element of one list, which names its elements by a key.
    Keyed,
    /// In a list without a key, or in a list inside another.
    Unkeyed,
}

/// What a field's `setting` option says.
#[derive(Debug, Clone, Default)]
struct Shown {
    required: bool,
    secret: bool,
    workspace: bool,
    board: bool,
    key: bool,
    /// What its text must match whole; empty: any text.
    pattern: String,
    /// A text that matches `pattern`.
    example: String,
}

/// `reason` at the field path `path`.
fn at(path: &[Step], reason: impl Into<String>) -> SchemaError {
    SchemaError {
        at: if path.is_empty() {
            "/".to_string()
        } else {
            names(path)
        },
        reason: reason.into(),
    }
}

fn refused(field: impl Into<String>, reason: impl Into<String>) -> ConfigError {
    ConfigError {
        field: field.into(),
        reason: reason.into(),
    }
}

impl ConfigType {
    /// The configuration message `manifest` names; none when it names
    /// none.
    ///
    /// # Errors
    /// Its descriptors do not read or lack the message, or a field is no
    /// setting: a map, `bytes`, a oneof, a well-known type, a message that
    /// holds itself, a secret that is not a string or is inside a list
    /// without a key or inside a list in another, a key outside a list's
    /// element or not a string, two keys of one element, a workspace that
    /// is not a string, a pattern on what is not a string, one that does
    /// not compile, one without an example that matches it, an example
    /// without a pattern.
    pub fn of(manifest: &Manifest) -> Result<Option<Self>, SchemaError> {
        if manifest.config_type.is_empty() {
            return Ok(None);
        }
        let pool = descriptors(&manifest.descriptors).map_err(|reason| at(&[], reason))?;
        let message = pool
            .get_message_by_name(&manifest.config_type)
            .ok_or_else(|| {
                at(
                    &[],
                    format!(
                        "{} is not a message of its descriptors",
                        manifest.config_type
                    ),
                )
            })?;
        let mut config = Self {
            message: message.clone(),
            setting: pool.get_extension_by_name(SETTING),
            secrets: Vec::new(),
        };
        let mut open = HashSet::new();
        config.fields(&message, &mut Vec::new(), Within::Outside, false, &mut open)?;
        Ok(Some(config))
    }

    /// Its full name.
    #[must_use]
    pub fn name(&self) -> &str {
        self.message.full_name()
    }

    /// The `setting` option of `field`.
    fn shown(&self, field: &FieldDescriptor) -> Shown {
        let Some(setting) = &self.setting else {
            return Shown::default();
        };
        let options = field.options();
        if !options.has_extension(setting) {
            return Shown::default();
        }
        let Field::Message(option) = options.get_extension(setting).into_owned() else {
            return Shown::default();
        };
        let flag = |name: &str| {
            option
                .get_field_by_name(name)
                .is_some_and(|v| v.as_bool() == Some(true))
        };
        let text = |name: &str| {
            option
                .get_field_by_name(name)
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default()
        };
        Shown {
            required: flag("required"),
            secret: flag("secret"),
            workspace: flag("workspace"),
            board: flag("board"),
            key: flag("key"),
            pattern: text("pattern"),
            example: text("example"),
        }
    }

    /// The field of `element` that names it in its list: none without one.
    ///
    /// # Errors
    /// It has two.
    fn key_of(
        &self,
        element: &MessageDescriptor,
        path: &[Step],
    ) -> Result<Option<FieldDescriptor>, SchemaError> {
        let mut keys = element.fields().filter(|f| self.shown(f).key);
        let key = keys.next();
        if keys.next().is_some() {
            return Err(at(path, "an element of a list has one key"));
        }
        Ok(key)
    }

    /// Check the fields of `message` at `path`, which lies `within` lists
    /// (`element`: it is a list's element), collecting the secrets.
    fn fields(
        &mut self,
        message: &MessageDescriptor,
        path: &mut Vec<Step>,
        within: Within,
        element: bool,
        open: &mut HashSet<String>,
    ) -> Result<(), SchemaError> {
        if !open.insert(message.full_name().to_string()) {
            return Err(at(path, format!("{} holds itself", message.full_name())));
        }
        for field in message.fields() {
            let key = match field.kind() {
                Kind::Message(inner) if field.is_list() => self.key_of(&inner, path)?,
                _ => None,
            };
            path.push(Step {
                field: field.clone(),
                key: key.clone(),
            });
            let shown = self.shown(&field);
            if field.is_map() {
                return Err(at(path, "a map is no setting"));
            }
            if field
                .containing_oneof()
                .is_some_and(|oneof| !oneof.is_synthetic())
            {
                return Err(at(path, "a oneof is no setting"));
            }
            let string = matches!(field.kind(), Kind::String);
            if shown.secret && (!string || field.is_list() || within == Within::Unkeyed) {
                return Err(at(
                    path,
                    "a secret is a string outside any list, or in an element of one list with a key",
                ));
            }
            if shown.key && (!string || field.is_list() || !element) {
                return Err(at(path, "a key is a string of a list's element"));
            }
            if shown.workspace && !string {
                return Err(at(path, "a workspace is a string"));
            }
            if shown.board && !string {
                return Err(at(path, "a board is a string"));
            }
            if shown.board && shown.workspace {
                return Err(at(path, "a setting is a workspace or a board, not both"));
            }
            declared(&shown.pattern, &shown.example, string).map_err(|why| at(path, why))?;
            match field.kind() {
                Kind::Bytes => return Err(at(path, "bytes are no setting")),
                Kind::Message(inner) if inner.package_name() == "google.protobuf" => {
                    return Err(at(path, format!("{} is no setting", inner.full_name())));
                }
                Kind::Message(inner) => {
                    let inner_within = match (field.is_list(), within, &key) {
                        (false, within, _) => within,
                        (true, Within::Outside, Some(_)) => Within::Keyed,
                        (true, _, _) => Within::Unkeyed,
                    };
                    self.fields(&inner, path, inner_within, field.is_list(), open)?;
                }
                _ => {}
            }
            if shown.secret {
                self.secrets.push(path.clone());
            }
            path.pop();
        }
        open.remove(message.full_name());
        Ok(())
    }

    /// `config` read as the message; an empty `Any` is an empty one.
    fn decode(&self, config: &Any) -> Result<DynamicMessage, ConfigError> {
        if config.type_url.is_empty() && config.value.is_empty() {
            return Ok(DynamicMessage::new(self.message.clone()));
        }
        let named = config
            .type_url
            .rsplit_once('/')
            .map_or(config.type_url.as_str(), |(_, name)| name);
        if named != self.name() {
            return Err(refused("", format!("is a {named}, not a {}", self.name())));
        }
        DynamicMessage::decode(self.message.clone(), config.value.as_ref())
            .map_err(|e| refused("", format!("does not decode as a {}: {e}", self.name())))
    }

    /// `message` as an `Any`.
    fn encode(&self, message: &DynamicMessage) -> Any {
        Any {
            type_url: format!("{TYPE_URL}{}", self.name()),
            value: message.encode_to_vec().into(),
        }
    }

    /// Check `config`: a message of this type, every required setting
    /// set, every workspace a workspace id.
    ///
    /// # Errors
    /// The first setting at fault and why.
    pub fn check(&self, config: &Any) -> Result<(), ConfigError> {
        let message = self.decode(config)?;
        self.check_message(&message, "")
    }

    fn check_message(&self, message: &DynamicMessage, prefix: &str) -> Result<(), ConfigError> {
        for field in message.descriptor().fields() {
            let named = if prefix.is_empty() {
                field.name().to_string()
            } else {
                format!("{prefix}.{}", field.name())
            };
            let shown = self.shown(&field);
            let value = message.get_field(&field);
            let set = match value.as_ref() {
                Field::List(items) => !items.is_empty(),
                _ => message.has_field(&field),
            };
            if shown.required && !set {
                return Err(refused(named, "is required"));
            }
            let items: Vec<(String, &Field)> = match value.as_ref() {
                Field::List(items) => items
                    .iter()
                    .enumerate()
                    .map(|(i, item)| (format!("{named}[{i}]"), item))
                    .collect(),
                _ if set => vec![(named.clone(), value.as_ref())],
                _ => Vec::new(),
            };
            if let (Field::List(elements), Kind::Message(inner)) = (value.as_ref(), field.kind())
                && let Some(key) = inner.fields().find(|f| self.shown(f).key)
            {
                check_keys(elements, &key, &named)?;
            }
            for (path, item) in items {
                match item {
                    Field::String(id) if shown.workspace && !uuid_text(id) => {
                        return Err(refused(path, "is a workspace id (a 36-char UUID)"));
                    }
                    Field::String(id) if shown.board && !id.is_empty() && !uuid_text(id) => {
                        return Err(refused(path, "is a board id (a 36-char UUID)"));
                    }
                    Field::String(text) if !shown.pattern.is_empty() => {
                        let pattern = whole(&shown.pattern).map_err(|why| refused(&path, why))?;
                        if !pattern.is_match(text) {
                            return Err(refused(
                                path,
                                format!("is like {} (it matches {})", shown.example, shown.pattern),
                            ));
                        }
                    }
                    Field::Message(inner) => self.check_message(inner, &path)?,
                    _ => {}
                }
            }
        }
        Ok(())
    }

    /// `config` read from its JSON form (the proto JSON mapping, proto
    /// field names), as a configuration file holds it; `null` is an empty
    /// one. It is not checked: a change may leave its secrets out.
    ///
    /// # Errors
    /// It is not one of this message.
    pub fn from_json(&self, config: &Value) -> Result<Any, ConfigError> {
        let message = match config {
            Value::Null => DynamicMessage::new(self.message.clone()),
            config => DynamicMessage::deserialize(self.message.clone(), config.clone())
                .map_err(|e| refused("", format!("is not a {}: {e}", self.name())))?,
        };
        Ok(self.encode(&message))
    }

    /// The JSON form of `config` (proto field names, unset fields left
    /// out), as a configuration file holds it.
    ///
    /// # Errors
    /// It is not a message of this type.
    pub fn to_json(&self, config: &Any) -> Result<Value, ConfigError> {
        let message = self.decode(config)?;
        let options = SerializeOptions::new().use_proto_field_name(true);
        message
            .serialize_with_options(serde_json::value::Serializer, &options)
            .map_err(|e| refused("", format!("has no JSON form: {e}")))
    }

    /// `config`'s field of message `full_name` (a grant the host reads),
    /// none when the message has no such field or leaves it unset.
    fn granted<T: prost::Message + Default>(
        &self,
        config: &Any,
        full_name: &str,
    ) -> Result<Option<T>, ConfigError> {
        let message = self.decode(config)?;
        let field = self
            .message
            .fields()
            .find(|f| matches!(f.kind(), Kind::Message(m) if m.full_name() == full_name));
        let Some(field) = field else {
            return Ok(None);
        };
        let Field::Message(grant) = message.get_field(&field).into_owned() else {
            return Ok(None);
        };
        grant
            .transcode_to()
            .map(Some)
            .map_err(|e| refused(field.name(), format!("is not a grant: {e}")))
    }

    /// What `config` grants the turns its module asks of the agent: its
    /// `pcx.module.v1.TurnGrant` field, none without one.
    ///
    /// # Errors
    /// It is not a message of this type.
    pub fn grant(&self, config: &Any) -> Result<TurnGrant, ConfigError> {
        Ok(self
            .granted::<wire::TurnGrant>(config, GRANT)?
            .map(TurnGrant::from)
            .unwrap_or_default())
    }

    /// The persons `config` lets its module act for: its
    /// `pcx.module.v1.PersonGrant` field, none without one (W29).
    ///
    /// # Errors
    /// It is not a message of this type.
    pub fn persons(&self, config: &Any) -> Result<Vec<String>, ConfigError> {
        Ok(self
            .granted::<wire::PersonGrant>(config, PERSONS)?
            .map(|grant| grant.persons)
            .unwrap_or_default())
    }
}

/// Whether `id` is a workspace id: a 36-char UUID.
fn uuid_text(id: &str) -> bool {
    id.len() == 36 && uuid::Uuid::try_parse(id).is_ok()
}

/// Check the keys of a list's `elements`: each names its element (1 to
/// 64 of a-z, 0-9, '-') and no two are the same.
fn check_keys(elements: &[Field], key: &FieldDescriptor, named: &str) -> Result<(), ConfigError> {
    let mut seen = HashSet::new();
    for (i, element) in elements.iter().enumerate() {
        let Field::Message(element) = element else {
            continue;
        };
        let name = element.get_field(key);
        let path = format!("{named}[{i}].{}", key.name());
        let Some(name) = name.as_str().filter(|n| is_slug(n)) else {
            return Err(refused(
                path,
                "names its element: 1 to 64 of a-z, 0-9 and '-'",
            ));
        };
        if !seen.insert(name.to_string()) {
            return Err(refused(path, format!("{name} names two elements")));
        }
    }
    Ok(())
}
