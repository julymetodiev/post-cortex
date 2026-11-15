// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A module's configuration (ARCHITECTURE section 7, settings): the
//! message its manifest names ([`ConfigType`]), with the one check of a
//! configuration and the rules for its secrets, so every host and the cloud
//! apply the same; and the JSON Schema subset of a view's forms
//! ([`FormSchema`], Module UI).

mod check;
mod document;
mod message;
mod pattern;

use std::fmt;

use serde_json::{Map, Value};

pub use document::{DeclaredKind, KindError, data_matches, descriptors};
pub use message::ConfigType;

use crate::{ModuleError, ModuleErrorKind};

/// The keywords of the subset.
const KEYWORDS: [&str; 10] = [
    "type",
    "format",
    "properties",
    "required",
    "enum",
    "items",
    "writeOnly",
    "title",
    "description",
    "default",
];

/// A parsed form schema: an object whose fields are of the subset.
#[derive(Debug, Clone, PartialEq)]
pub struct FormSchema {
    root: Object,
}

/// One schema node.
#[derive(Debug, Clone, PartialEq)]
struct Node {
    kind: Kind,
    /// The values `enum` allows.
    choices: Option<Vec<Value>>,
    /// What its string names (`format`).
    format: Option<Format>,
}

/// What a string setting names (`format`); an app offers its values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    /// A workspace of the host, by its id (a 36-char UUID).
    Workspace,
    /// A board, by its id (a 36-char UUID).
    Board,
    /// Text of several lines (a file, instructions).
    Multiline,
}

/// What a node holds.
#[derive(Debug, Clone, PartialEq)]
enum Kind {
    Object(Object),
    String,
    Number,
    Integer,
    Boolean,
    Array(Box<Node>),
}

/// An object's settings.
#[derive(Debug, Clone, PartialEq)]
struct Object {
    properties: Vec<(String, Node)>,
    required: Vec<String>,
}

/// A schema outside the subset, or a configuration message outside the
/// settings' rules; the manifest that carries it is refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("the schema at {at}: {reason}")]
pub struct SchemaError {
    /// Where in the schema (`properties.token.writeOnly`, a field path;
    /// `/` for the root).
    pub at: String,
    /// What is wrong.
    pub reason: String,
}

/// A configuration its message refuses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    /// The setting at fault (`token`, `slack.channels[2]`; empty: the whole
    /// configuration).
    pub field: String,
    /// What is wrong.
    pub reason: String,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.field.is_empty() {
            write!(f, "the configuration {}", self.reason)
        } else {
            write!(f, "setting {} {}", self.field, self.reason)
        }
    }
}

impl std::error::Error for ConfigError {}

impl From<ConfigError> for ModuleError {
    /// `InvalidArgument` naming the field.
    fn from(e: ConfigError) -> Self {
        let field = e.field.clone();
        ModuleError::new(ModuleErrorKind::InvalidArgument, e.to_string()).with_field(field)
    }
}

impl FormSchema {
    /// Read `schema`, refusing anything outside the subset: another keyword,
    /// a `format` other than `workspace` on a string, a node without a type, `writeOnly` on an object, an array or inside
    /// an array, a `default` or `enum` value its node refuses, a setting
    /// name with `.`, `[` or `]` (they write paths).
    ///
    /// # Errors
    /// Where and why the schema is outside the subset.
    pub fn parse(schema: &Map<String, Value>) -> Result<Self, SchemaError> {
        let mut parser = Parser;
        let root = match parser.node(schema, &mut Vec::new(), false)? {
            Node {
                kind: Kind::Object(root),
                choices: None,
                format: None,
            } => root,
            _ => return Err(at(&[], "the configuration is an object")),
        };
        Ok(Self { root })
    }

    /// The names of the top-level fields, in schema order.
    pub fn fields(&self) -> impl Iterator<Item = &str> {
        self.root.properties.iter().map(|(name, _)| name.as_str())
    }
}

/// `reason` at the schema path `path`.
fn at(path: &[String], reason: impl Into<String>) -> SchemaError {
    SchemaError {
        at: if path.is_empty() {
            "/".to_string()
        } else {
            path.join(".")
        },
        reason: reason.into(),
    }
}

/// The reader of one schema.
struct Parser;

impl Parser {
    /// The node `schema` at schema path `path`; `in_array` when it is an
    /// array's item or inside one.
    fn node(
        &mut self,
        schema: &Map<String, Value>,
        path: &mut Vec<String>,
        in_array: bool,
    ) -> Result<Node, SchemaError> {
        if let Some(keyword) = schema.keys().find(|k| !KEYWORDS.contains(&k.as_str())) {
            return Err(at(path, format!("the keyword {keyword} is not supported")));
        }
        for keyword in ["title", "description"] {
            if schema.get(keyword).is_some_and(|v| !v.is_string()) {
                return Err(at(path, format!("{keyword} is a string")));
            }
        }
        let kind = match schema.get("type") {
            Some(Value::String(name)) => name.as_str(),
            Some(_) => return Err(at(path, "type is one type name")),
            None => return Err(at(path, "a setting needs a type")),
        };
        let kind = match kind {
            "object" => Kind::Object(self.object(schema, path, in_array)?),
            "array" => Kind::Array(Box::new(self.items(schema, path)?)),
            "string" => Kind::String,
            "number" => Kind::Number,
            "integer" => Kind::Integer,
            "boolean" => Kind::Boolean,
            other => return Err(at(path, format!("the type {other} is not supported"))),
        };
        let structural = matches!(kind, Kind::Object(_) | Kind::Array(_));
        for keyword in ["properties", "required"] {
            if !matches!(kind, Kind::Object(_)) && schema.contains_key(keyword) {
                return Err(at(path, format!("{keyword} belongs to an object")));
            }
        }
        if !matches!(kind, Kind::Array(_)) && schema.contains_key("items") {
            return Err(at(path, "items belongs to an array"));
        }
        match schema.get("writeOnly") {
            None | Some(Value::Bool(false)) => {}
            Some(Value::Bool(true)) if structural => {
                return Err(at(path, "writeOnly is for a single value"));
            }
            Some(Value::Bool(true)) if in_array => {
                return Err(at(path, "writeOnly is not allowed inside an array"));
            }
            Some(Value::Bool(true)) => {}
            Some(_) => return Err(at(path, "writeOnly is true or false")),
        }
        let mut node = Node {
            kind,
            choices: None,
            format: None,
        };
        if let Some(format) = schema.get("format") {
            node.format = Some(format_of(&node, format, path)?);
        }
        if let Some(choices) = schema.get("enum") {
            node.choices = Some(choices_of(&node, choices, path)?);
        }
        if let Some(default) = schema.get("default")
            && let Err(e) = node.check(default, &mut String::new())
        {
            return Err(at(path, format!("default: {e}")));
        }
        Ok(node)
    }

    /// The settings of an object node.
    fn object(
        &mut self,
        schema: &Map<String, Value>,
        path: &mut Vec<String>,
        in_array: bool,
    ) -> Result<Object, SchemaError> {
        let mut properties = Vec::new();
        match schema.get("properties") {
            None => {}
            Some(Value::Object(props)) => {
                for (name, prop) in props {
                    if name.is_empty() || name.contains(['.', '[', ']']) {
                        return Err(at(
                            path,
                            format!("the setting name {name:?} is not allowed"),
                        ));
                    }
                    let Value::Object(prop) = prop else {
                        return Err(at(path, format!("the setting {name} is a schema object")));
                    };
                    path.push("properties".to_string());
                    path.push(name.clone());
                    let node = self.node(prop, path, in_array);
                    path.truncate(path.len() - 2);
                    properties.push((name.clone(), node?));
                }
            }
            Some(_) => return Err(at(path, "properties is an object")),
        }
        let required = match schema.get("required") {
            None => Vec::new(),
            Some(Value::Array(names)) => {
                let mut required = Vec::with_capacity(names.len());
                for name in names {
                    match name {
                        Value::String(n) if properties.iter().any(|(p, _)| p == n) => {
                            required.push(n.clone());
                        }
                        Value::String(n) => {
                            return Err(at(path, format!("required names {n}, not a setting")));
                        }
                        _ => return Err(at(path, "required is a list of setting names")),
                    }
                }
                required
            }
            Some(_) => return Err(at(path, "required is a list of setting names")),
        };
        Ok(Object {
            properties,
            required,
        })
    }

    /// The item node of an array node.
    fn items(
        &mut self,
        schema: &Map<String, Value>,
        path: &mut Vec<String>,
    ) -> Result<Node, SchemaError> {
        let Some(Value::Object(items)) = schema.get("items") else {
            return Err(at(path, "an array needs items, a schema object"));
        };
        path.push("items".to_string());
        let node = self.node(items, path, true);
        path.pop();
        node
    }
}

/// What a `format` says its string names.
fn format_of(node: &Node, format: &Value, path: &[String]) -> Result<Format, SchemaError> {
    if !matches!(node.kind, Kind::String) {
        return Err(at(path, "format is for a string"));
    }
    match format {
        Value::String(name) if name == "workspace" => Ok(Format::Workspace),
        Value::String(name) if name == "board" => Ok(Format::Board),
        Value::String(name) if name == "multiline" => Ok(Format::Multiline),
        Value::String(name) => Err(at(path, format!("the format {name} is not supported"))),
        _ => Err(at(path, "format is a name")),
    }
}

/// The values an `enum` allows, each one its node takes.
fn choices_of(node: &Node, choices: &Value, path: &[String]) -> Result<Vec<Value>, SchemaError> {
    if !matches!(node.kind, Kind::String | Kind::Number | Kind::Integer) {
        return Err(at(path, "enum is for a string or a number"));
    }
    let Value::Array(choices) = choices else {
        return Err(at(path, "enum is a list of values"));
    };
    if choices.is_empty() {
        return Err(at(path, "enum lists at least one value"));
    }
    for choice in choices {
        if let Err(e) = node.check(choice, &mut String::new()) {
            return Err(at(path, format!("enum: {e}")));
        }
    }
    Ok(choices.clone())
}

#[cfg(test)]
mod tests;
