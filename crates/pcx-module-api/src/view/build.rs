// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Building a view's tree from JSON: the components, sources, actions and
//! forms a module's view is made of, so every module writes them the same.

use serde_json::{Map, Value, json};

use crate::json;
use crate::view::{Action, Component, Direction, Fields, Form, Source, Stack, component};

/// A component of `kind`.
#[must_use]
pub fn node(kind: component::Kind) -> Component {
    Component { kind: Some(kind) }
}

/// The components `children`, one under another.
#[must_use]
pub fn stack(children: Vec<Component>) -> Component {
    node(component::Kind::Stack(Stack {
        direction: Direction::Column.into(),
        children,
    }))
}

/// A source: the RPC at `rpc` called with `request` (an object in the proto
/// JSON mapping; any other value: none), its response read at `pointer`.
#[must_use]
pub fn source(rpc: &str, request: Value, pointer: &str) -> Source {
    Source {
        rpc: rpc.to_string(),
        request: request.as_object().cloned().map(json::object_to_wire),
        pointer: pointer.to_string(),
    }
}

/// An action labelled `label` calling the RPC at `rpc` with `request`;
/// with `confirm`, it changes state and asks it first.
#[must_use]
pub fn action(label: &str, rpc: &str, request: Value, confirm: Option<&str>) -> Action {
    Action {
        label: label.to_string(),
        rpc: rpc.to_string(),
        request: request.as_object().cloned().map(json::object_to_wire),
        changes: confirm.is_some(),
        confirm: confirm.unwrap_or_default().to_string(),
        form: None,
        settings: false,
    }
}

/// An action labelled `label` that opens the module's settings form.
#[must_use]
pub fn settings(label: &str) -> Action {
    Action {
        label: label.to_string(),
        settings: true,
        ..Action::default()
    }
}

/// `action` asking for the fields `properties` (`required` of them) in a
/// dialog first; in a row action each starts from the row's field of the
/// same name.
#[must_use]
pub fn asking(action: Action, properties: Value, required: &[&str]) -> Action {
    Action {
        form: Some(json::object_to_wire(schema(properties, required))),
        ..action
    }
}

/// An object schema of `properties`, `required` of them.
fn schema(properties: Value, required: &[&str]) -> Map<String, Value> {
    Map::from_iter([
        ("type".to_string(), json!("object")),
        ("properties".to_string(), properties),
        ("required".to_string(), json!(required)),
    ])
}

/// A form of the fields `properties` (schemas of the settings subset),
/// `required` of them, sent by `submit`, shown in place.
#[must_use]
pub fn form(properties: Value, required: &[&str], submit: Action) -> Component {
    node(component::Kind::Form(Form {
        schema: Some(json::object_to_wire(schema(properties, required))),
        values: None,
        submit: Some(submit),
        open: String::new(),
    }))
}

/// A button `open` that opens a form of `properties` (`required` of
/// them) in a dialog, its first values from `values` (none: each field's
/// default), sent by `submit`.
#[must_use]
pub fn dialog(
    open: &str,
    properties: Value,
    required: &[&str],
    values: Option<Source>,
    submit: Action,
) -> Component {
    node(component::Kind::Form(Form {
        schema: Some(json::object_to_wire(schema(properties, required))),
        values,
        submit: Some(submit),
        open: open.to_string(),
    }))
}

/// The components `children` side by side (a bar of buttons).
#[must_use]
pub fn bar(children: Vec<Component>) -> Component {
    node(component::Kind::Stack(Stack {
        direction: Direction::Row.into(),
        children,
    }))
}

/// A text field titled `title`.
#[must_use]
pub fn text(title: &str) -> Value {
    json!({ "type": "string", "title": title })
}

/// A text field of several lines titled `title`.
#[must_use]
pub fn multiline(title: &str) -> Value {
    json!({ "type": "string", "format": "multiline", "title": title })
}

/// A field of lines (a list of text) titled `title`.
#[must_use]
pub fn lines(title: &str) -> Value {
    json!({ "type": "array", "title": title, "items": { "type": "string" } })
}

/// A group of fields titled `title`: an object of `properties`,
/// `required` of them, as a message field of a request is.
#[must_use]
pub fn group(title: &str, properties: Value, required: &[&str]) -> Value {
    let mut group = schema(properties, required);
    group.insert("title".to_string(), json!(title));
    Value::Object(group)
}

/// The object `source` reads, field by field, with `actions` under it.
#[must_use]
pub fn fields(source: Source, actions: Vec<Action>) -> Component {
    node(component::Kind::Fields(Fields {
        source: Some(source),
        actions,
    }))
}
