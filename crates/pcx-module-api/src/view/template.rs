// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The substitutions of a view's requests: a string value that is exactly
//! `{{row.<field>}}`, `{{form.<field>}}`, `{{drop.<lane|before|group>}}`
//! (a lanes' move, W24) or `{{images}}` (an images' attach, W24), each
//! only where it has something to stand for.

use serde_json::Value;

use crate::view::{Checker, ViewError};

/// Where a template is: what its `{{...}}` values may name.
#[derive(Clone, Copy)]
pub struct Slot<'a> {
    /// `{{row.<field>}}`: in a row action, and anywhere inside a detail.
    pub row: bool,
    /// `{{form.<field>}}` of these fields (or a path under one, as
    /// `skill.description`): in a form's submit, or an action that asks
    /// for them.
    pub form: Option<&'a [String]>,
    /// `{{drop.lane}}`, `{{drop.before}}` and `{{drop.group}}`: in a
    /// lanes' move.
    pub drop: bool,
    /// `{{images}}`: in an images' attach.
    pub images: bool,
}

impl Slot<'static> {
    /// Nothing to stand for.
    pub const NONE: Self = Self {
        row: false,
        form: None,
        drop: false,
        images: false,
    };
}

/// What a drop names.
const DROP: [&str; 3] = ["lane", "before", "group"];

/// `{{scope.field}}` as `(scope, field)` (`{{images}}`: field empty), when
/// `s` is exactly a substitution.
fn placeholder(s: &str) -> Option<(&str, &str)> {
    let inner = s.strip_prefix("{{")?.strip_suffix("}}")?;
    if inner == "images" {
        return Some(("images", ""));
    }
    let (scope, field) = inner.split_once('.')?;
    (matches!(scope, "row" | "form" | "drop") && !field.is_empty()).then_some((scope, field))
}

impl Checker<'_> {
    /// Every substitution in `value` fits `slot`.
    pub fn template(&self, value: &Value, at: &str, slot: Slot<'_>) -> Result<(), ViewError> {
        match value {
            Value::Object(fields) => fields.values().try_for_each(|v| self.template(v, at, slot)),
            Value::Array(items) => items.iter().try_for_each(|v| self.template(v, at, slot)),
            Value::String(s) => match (placeholder(s), slot.form) {
                (None, _) => Ok(()),
                (Some(("row", _)), _) if slot.row => Ok(()),
                (Some(("drop", field)), _) if slot.drop && DROP.contains(&field) => Ok(()),
                (Some(("images", _)), _) if slot.images => Ok(()),
                (Some(("form", field)), Some(fields))
                    if fields.iter().any(|f| field.split('.').next() == Some(f)) =>
                {
                    Ok(())
                }
                (Some(("form", field)), Some(_)) => {
                    Err(self.refused(at, format!("{s}: the form has no field {field}")))
                }
                (Some(_), _) => Err(self.refused(at, format!("{s} has nothing to stand for here"))),
            },
            _ => Ok(()),
        }
    }
}
