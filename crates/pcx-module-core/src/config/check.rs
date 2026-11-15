// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The check of a value against its form schema node (its `default` and
//! `enum` values).

use serde_json::{Map, Value};

use crate::config::{ConfigError, Format, Kind, Node, Object};

/// `reason` about the setting at `field`.
fn refused(field: &str, reason: &str) -> ConfigError {
    ConfigError {
        field: field.to_string(),
        reason: reason.to_string(),
    }
}

impl Node {
    /// Check `value`, the setting at `field`.
    pub fn check(&self, value: &Value, field: &mut String) -> Result<(), ConfigError> {
        let fits = match (&self.kind, value) {
            (Kind::Object(object), Value::Object(fields)) => return object.check(fields, field),
            (Kind::Array(item), Value::Array(items)) => {
                for (i, value) in items.iter().enumerate() {
                    let len = field.len();
                    field.push_str(&format!("[{i}]"));
                    item.check(value, field)?;
                    field.truncate(len);
                }
                return Ok(());
            }
            (Kind::String, Value::String(_)) | (Kind::Boolean, Value::Bool(_)) => true,
            (Kind::Number, Value::Number(_)) => true,
            (Kind::Integer, Value::Number(n)) => n.is_i64() || n.is_u64(),
            _ => false,
        };
        if !fits {
            return Err(refused(field, &format!("is {}", self.kind.name())));
        }
        if let Some(choices) = &self.choices
            && !choices.contains(value)
        {
            let listed: Vec<String> = choices.iter().map(Value::to_string).collect();
            return Err(refused(field, &format!("is one of {}", listed.join(", "))));
        }
        match (self.format, value) {
            (Some(Format::Workspace), Value::String(id))
                if id.len() != 36 || uuid::Uuid::try_parse(id).is_err() =>
            {
                Err(refused(field, "is a workspace id (a 36-char UUID)"))
            }
            (Some(Format::Board), Value::String(id))
                if !id.is_empty() && (id.len() != 36 || uuid::Uuid::try_parse(id).is_err()) =>
            {
                Err(refused(field, "is a board id (a 36-char UUID)"))
            }
            _ => Ok(()),
        }
    }
}

impl Object {
    /// Check `fields`, the object at `field`.
    fn check(&self, fields: &Map<String, Value>, field: &mut String) -> Result<(), ConfigError> {
        for name in &self.required {
            if !fields.contains_key(name) {
                let len = field.len();
                push_key(field, name);
                let e = refused(field, "is required");
                field.truncate(len);
                return Err(e);
            }
        }
        for (name, value) in fields {
            let len = field.len();
            push_key(field, name);
            match self.properties.iter().find(|(p, _)| p == name) {
                Some((_, node)) => node.check(value, field)?,
                None => return Err(refused(field, "is not a setting of this module")),
            }
            field.truncate(len);
        }
        Ok(())
    }
}

/// Append the key `name` to the setting path `field`.
fn push_key(field: &mut String, name: &str) {
    if !field.is_empty() {
        field.push('.');
    }
    field.push_str(name);
}

impl Kind {
    /// What the kind is, for an error.
    fn name(&self) -> &'static str {
        match self {
            Kind::Object(_) => "an object",
            Kind::Array(_) => "a list",
            Kind::String => "a string",
            Kind::Number => "a number",
            Kind::Integer => "a whole number",
            Kind::Boolean => "true or false",
        }
    }
}
