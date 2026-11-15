// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A module's views (ARCHITECTURE section 7, Module UI levels 2 and 3): the
//! `pcx.view.v1` tree a manifest declares, and the one check of it against
//! the module's RPCs and their messages, so every host and the cloud refuse
//! the same views.

use std::collections::{HashMap, HashSet};
use std::fmt;

use component::Kind;
pub use post_cortex_proto::ui::component;
pub use post_cortex_proto::ui::markdown;
use post_cortex_proto::ui::markdown::Content;
pub use post_cortex_proto::ui::view::Body;
pub use post_cortex_proto::ui::{
    Action, Card, Chart, ChartKind, Chat, Column, Component, Direction, Fields, Form, Format,
    Graph, Images, Lanes, List, Markdown, Source, Stack, Stat, Table, View,
};
use prost_reflect::MethodDescriptor;
use serde_json::Value;

use crate::{Capability, FormSchema, Manifest, json};

pub mod build;
mod chat;
mod lanes;
mod shape;
mod template;

use template::Slot;

/// The MIME type of a view's HTML resource (MCP Apps).
pub const HTML_MIME: &str = "text/html;profile=mcp-app";

/// A view its module cannot have; the module is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewError {
    /// The view's id (empty when it has none).
    pub view: String,
    /// The component at fault (`root.stack[1].table`; empty: the view).
    pub at: String,
    /// What is wrong.
    pub reason: String,
}

impl fmt::Display for ViewError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.view.is_empty(), self.at.is_empty()) {
            (true, _) => write!(f, "a view {}", self.reason),
            (false, true) => write!(f, "view {} {}", self.view, self.reason),
            (false, false) => write!(f, "view {} at {}: {}", self.view, self.at, self.reason),
        }
    }
}

impl std::error::Error for ViewError {}

/// Check the views of `manifest` against its RPCs: ids unique, every
/// source and action naming one of its `rpcs` with a request whose keys
/// are fields of its request message, every pointer a JSON Pointer to a
/// field of its response, every `{{form.x}}` a field of its form and
/// `{{row.x}}` only in row actions and inside a row's `detail`, every enum
/// set, a chat with the host's agent or the module's own turns; an HTML view needs `ui_html` and a `ui://<module id>/` URI.
///
/// # Errors
/// The first view at fault, where and why.
pub fn check(manifest: &Manifest) -> Result<(), ViewError> {
    let rpcs = shape::methods(manifest).map_err(|reason| ViewError {
        view: String::new(),
        at: String::new(),
        reason,
    })?;
    let settings = !manifest.config_type.is_empty();
    let html = manifest
        .capabilities
        .contains(&i32::from(Capability::UiHtml));
    let mut ids = HashSet::new();
    for view in &manifest.views {
        let refused = |at: &str, reason: &str| ViewError {
            view: view.id.clone(),
            at: at.to_string(),
            reason: reason.to_string(),
        };
        if view.id.trim().is_empty() {
            return Err(refused("", "has no id"));
        }
        if !ids.insert(view.id.as_str()) {
            return Err(refused("", "is declared twice"));
        }
        if view.title.trim().is_empty() {
            return Err(refused("", "has no title"));
        }
        match &view.body {
            None => return Err(refused("", "has neither a root nor a resource")),
            Some(Body::Root(root)) => {
                let checker = Checker {
                    rpcs: &rpcs,
                    view: &view.id,
                    detail: false,
                    settings,
                    module: &manifest.id,
                    turns: manifest.turns,
                };
                checker.component(root, &mut "root".to_string())?;
            }
            Some(Body::ResourceUri(uri)) => {
                if !html {
                    return Err(refused("", "is HTML, which needs the ui_html capability"));
                }
                let prefix = format!("ui://{}/", manifest.id);
                if uri.strip_prefix(&prefix).is_none_or(str::is_empty) {
                    return Err(refused("", &format!("names {uri}, not a {prefix}<path>")));
                }
            }
        }
    }
    Ok(())
}

/// The check of one view's tree.
struct Checker<'a> {
    /// The module's RPCs, by their gRPC path.
    rpcs: &'a HashMap<&'a str, MethodDescriptor>,
    view: &'a str,
    /// Inside a row's detail, where `{{row.<field>}}` is the picked row's.
    detail: bool,
    /// Whether the module has settings (a `config_schema`).
    settings: bool,
    /// The module's id.
    module: &'a str,
    /// Whether the module runs turns of its own.
    turns: bool,
}

impl Checker<'_> {
    fn refused(&self, at: &str, reason: impl Into<String>) -> ViewError {
        ViewError {
            view: self.view.to_string(),
            at: at.to_string(),
            reason: reason.into(),
        }
    }

    /// Check `component`, at `at`.
    fn component(&self, component: &Component, at: &mut String) -> Result<(), ViewError> {
        let Some(kind) = &component.kind else {
            return Err(self.refused(at, "is no component"));
        };
        let len = at.len();
        at.push('.');
        at.push_str(kind_name(kind));
        let at_kind = at.clone();
        let here = at_kind.as_str();
        match kind {
            Kind::Stack(stack) => {
                if Direction::try_from(stack.direction).is_err()
                    || stack.direction == i32::from(Direction::Unspecified)
                {
                    return Err(self.refused(here, "has no direction"));
                }
                if stack.children.is_empty() {
                    return Err(self.refused(here, "has no children"));
                }
                for (i, child) in stack.children.iter().enumerate() {
                    let child_len = at.len();
                    at.push_str(&format!("[{i}]"));
                    self.component(child, at)?;
                    at.truncate(child_len);
                }
            }
            Kind::List(list) => {
                self.source(list.source.as_ref(), here)?;
                self.named(&list.title_field, here, "title_field")?;
                self.actions(&list.row_actions, here, self.row_actions())?;
                self.detail(list.detail.as_deref(), at)?;
            }
            Kind::Lanes(lanes) => {
                self.lanes(lanes, here)?;
                self.detail(lanes.detail.as_deref(), at)?;
            }
            Kind::Images(images) => self.images(images, here)?,
            Kind::Chat(chat) => self.chat(chat, here)?,
            Kind::Table(table) => {
                self.source(table.source.as_ref(), here)?;
                if table.columns.is_empty() {
                    return Err(self.refused(here, "has no columns"));
                }
                for column in &table.columns {
                    self.named(&column.field, here, "a column's field")?;
                    self.named(&column.label, here, "a column's label")?;
                    self.format(column.format, here)?;
                }
                self.actions(&table.row_actions, here, self.row_actions())?;
                self.detail(table.detail.as_deref(), at)?;
            }
            Kind::Form(form) => self.form(form, here)?,
            Kind::Fields(fields) => {
                self.source(fields.source.as_ref(), here)?;
                self.actions(
                    &fields.actions,
                    here,
                    Slot {
                        row: self.detail,
                        form: None,
                        ..Slot::NONE
                    },
                )?;
            }
            Kind::Stat(stat) => {
                self.named(&stat.label, here, "label")?;
                self.source(stat.source.as_ref(), here)?;
                self.format(stat.format, here)?;
            }
            Kind::Chart(chart) => {
                if ChartKind::try_from(chart.kind).is_err()
                    || chart.kind == i32::from(ChartKind::Unspecified)
                {
                    return Err(self.refused(here, "has no kind"));
                }
                self.source(chart.source.as_ref(), here)?;
                self.named(&chart.x_field, here, "x_field")?;
                if chart.y_fields.is_empty() || chart.y_fields.iter().any(|f| f.trim().is_empty()) {
                    return Err(self.refused(here, "needs y_fields, each named"));
                }
            }
            Kind::Markdown(markdown) => match &markdown.content {
                None => return Err(self.refused(here, "has neither text nor a source")),
                Some(Content::Text(_)) => {}
                Some(Content::Source(source)) => self.source(Some(source), here)?,
            },
            Kind::Graph(graph) => {
                self.source(graph.source.as_ref(), here)?;
                for (field, name) in [
                    (&graph.nodes_field, "nodes_field"),
                    (&graph.edges_field, "edges_field"),
                    (&graph.id_field, "id_field"),
                    (&graph.label_field, "label_field"),
                    (&graph.from_field, "from_field"),
                    (&graph.to_field, "to_field"),
                ] {
                    self.named(field, here, name)?;
                }
            }
        }
        at.truncate(len);
        Ok(())
    }

    /// A form: its schema in the settings subset, its first values, its
    /// submit taking its own fields.
    fn form(&self, form: &Form, at: &str) -> Result<(), ViewError> {
        let schema = form
            .schema
            .clone()
            .ok_or_else(|| self.refused(at, "has no schema"))?;
        let schema = FormSchema::parse(&json::object_from_wire(schema))
            .map_err(|e| self.refused(at, e.to_string()))?;
        let fields: Vec<String> = schema.fields().map(String::from).collect();
        if let Some(values) = &form.values {
            self.source(Some(values), at)?;
        }
        let submit = form
            .submit
            .as_ref()
            .ok_or_else(|| self.refused(at, "has no submit"))?;
        if submit.form.is_some() {
            return Err(self.refused(at, "a form's submit asks no fields of its own"));
        }
        if submit.settings {
            return Err(self.refused(at, "a form's submit calls a tool, not the settings"));
        }
        self.action(
            submit,
            at,
            Slot {
                row: self.detail,
                form: Some(&fields),
                ..Slot::NONE
            },
        )
    }

    /// What a row action's template may name.
    fn row_actions(&self) -> Slot<'static> {
        Slot {
            row: true,
            form: None,
            ..Slot::NONE
        }
    }

    /// A row's detail, when it has one, at `at.detail`: inside it
    /// `{{row.<field>}}` is the picked row's.
    fn detail(&self, detail: Option<&Component>, at: &mut String) -> Result<(), ViewError> {
        let Some(detail) = detail else {
            return Ok(());
        };
        let len = at.len();
        at.push_str(".detail");
        let inner = Checker {
            rpcs: self.rpcs,
            view: self.view,
            detail: true,
            settings: self.settings,
            module: self.module,
            turns: self.turns,
        };
        inner.component(detail, at)?;
        at.truncate(len);
        Ok(())
    }

    /// A source naming one of the module's RPCs, with a pointer into its
    /// response.
    fn source(&self, source: Option<&Source>, at: &str) -> Result<(), ViewError> {
        let source = source.ok_or_else(|| self.refused(at, "has no source"))?;
        let method = self.rpc(&source.rpc, at)?;
        shape::pointer(&method.output(), &source.pointer).map_err(|e| self.refused(at, e))?;
        self.request(
            &method,
            source.request.as_ref(),
            at,
            Slot {
                row: self.detail,
                form: None,
                ..Slot::NONE
            },
        )
    }

    /// A request of `method` whose substitutions fit `slot`.
    fn request(
        &self,
        method: &MethodDescriptor,
        request: Option<&pbjson_types::Struct>,
        at: &str,
        slot: Slot<'_>,
    ) -> Result<(), ViewError> {
        let Some(request) = request else {
            return Ok(());
        };
        let request = json::object_from_wire(request.clone());
        shape::request(&method.input(), &request)
            .map_err(|e| self.refused(at, format!("{}: {e}", method.full_name())))?;
        self.template(&Value::Object(request), at, slot)
    }

    fn actions(&self, actions: &[Action], at: &str, slot: Slot<'_>) -> Result<(), ViewError> {
        actions.iter().try_for_each(|a| self.action(a, at, slot))
    }

    /// An action: a label, one of the module's RPCs, a request whose
    /// substitutions fit `slot` (and the fields it asks for, when it asks
    /// any: their schema in the settings subset), a question when it
    /// changes state; or a label only, opening the settings of a module
    /// that has them.
    fn action(&self, action: &Action, at: &str, slot: Slot<'_>) -> Result<(), ViewError> {
        self.named(&action.label, at, "an action's label")?;
        if action.settings {
            return self.settings_action(action, at);
        }
        let method = self.rpc(&action.rpc, at)?;
        if action.changes && action.confirm.trim().is_empty() {
            return Err(self.refused(
                at,
                format!("action {} changes state but asks nothing", action.label),
            ));
        }
        let asked: Option<Vec<String>> = match &action.form {
            Some(schema) => Some(
                FormSchema::parse(&json::object_from_wire(schema.clone()))
                    .map_err(|e| self.refused(at, format!("action {}: {e}", action.label)))?
                    .fields()
                    .map(String::from)
                    .collect(),
            ),
            None => None,
        };
        let slot = match &asked {
            Some(fields) => Slot {
                form: Some(fields),
                ..slot
            },
            None => slot,
        };
        self.request(&method, action.request.as_ref(), at, slot)
    }

    /// An action opening the module's settings: nothing but its label, of
    /// a module with settings.
    fn settings_action(&self, action: &Action, at: &str) -> Result<(), ViewError> {
        if !self.settings {
            return Err(self.refused(
                at,
                format!(
                    "action {} opens the settings of a module that has none",
                    action.label
                ),
            ));
        }
        if !action.rpc.is_empty()
            || action.request.is_some()
            || action.form.is_some()
            || action.changes
            || !action.confirm.is_empty()
        {
            return Err(self.refused(
                at,
                format!(
                    "action {} opens the settings: it has a label only",
                    action.label
                ),
            ));
        }
        Ok(())
    }

    /// The method of `rpc`, one the module's manifest names.
    fn rpc(&self, rpc: &str, at: &str) -> Result<MethodDescriptor, ViewError> {
        self.rpcs.get(rpc).cloned().ok_or_else(|| {
            self.refused(
                at,
                format!("names {rpc}, which is not an RPC of the module"),
            )
        })
    }

    fn named(&self, value: &str, at: &str, what: &str) -> Result<(), ViewError> {
        if value.trim().is_empty() {
            Err(self.refused(at, format!("has no {what}")))
        } else {
            Ok(())
        }
    }

    fn format(&self, format: i32, at: &str) -> Result<(), ViewError> {
        match Format::try_from(format) {
            Ok(Format::Unspecified) | Err(_) => {
                Err(self.refused(at, "has a value without a format"))
            }
            Ok(_) => Ok(()),
        }
    }
}

/// The name of a component kind, as `at` paths write it.
fn kind_name(kind: &Kind) -> &'static str {
    match kind {
        Kind::Stack(_) => "stack",
        Kind::List(_) => "list",
        Kind::Table(_) => "table",
        Kind::Form(_) => "form",
        Kind::Stat(_) => "stat",
        Kind::Chart(_) => "chart",
        Kind::Markdown(_) => "markdown",
        Kind::Graph(_) => "graph",
        Kind::Lanes(_) => "lanes",
        Kind::Fields(_) => "fields",
        Kind::Images(_) => "images",
        Kind::Chat(_) => "chat",
    }
}

#[cfg(test)]
mod tests;
