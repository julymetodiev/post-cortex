// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What a field's values must be beside their type (docs/modules/workflow.md,
//! data blocks): bounds, lengths, a pattern, a set of values; checked against
//! the field's type when a schema is read, and every one a value breaks
//! found, a map's or a message's fields by a schema's.

use std::cmp::Ordering;

use prost_reflect::{DynamicMessage, ReflectMessage};

use regex::{Regex, RegexBuilder};

use crate::schema::Field;
use crate::types::Type;
use crate::value::Value;

/// Bytes of a pattern at most.
const MAX_PATTERN: usize = 1024;

/// Bytes a pattern's compiled program may take at most.
const PATTERN_SIZE: usize = 1 << 20;

/// What a field's values must be; each part unset bounds nothing.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Constraints {
    /// The least value.
    pub minimum: Option<Bound>,
    /// The greatest value.
    pub maximum: Option<Bound>,
    /// The least length (characters, bytes, items, entries).
    pub min_length: Option<u64>,
    /// The greatest length.
    pub max_length: Option<u64>,
    /// What text matches whole.
    pub pattern: Option<Pattern>,
    /// The values it may be; empty: any.
    pub one_of: Vec<Bound>,
}

/// A value a constraint names, with its JSON (a model's JSON Schema
/// carries it).
#[derive(Debug, Clone, PartialEq)]
pub struct Bound {
    value: Value,
    json: serde_json::Value,
}

impl Bound {
    /// `value` as a bound.
    ///
    /// # Errors
    /// JSON does not carry it (a double that is not finite); why.
    pub fn new(value: Value) -> Result<Self, String> {
        let json = value.json()?;
        Ok(Self { value, json })
    }

    /// The value.
    #[must_use]
    pub fn value(&self) -> &Value {
        &self.value
    }

    /// Its JSON.
    #[must_use]
    pub fn json(&self) -> &serde_json::Value {
        &self.json
    }
}

/// A pattern text matches whole, compiled.
#[derive(Debug, Clone)]
pub struct Pattern {
    source: String,
    regex: Regex,
}

impl PartialEq for Pattern {
    fn eq(&self, other: &Self) -> bool {
        self.source == other.source
    }
}

impl Pattern {
    /// What it was compiled from.
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    /// `source` compiled to match a whole text.
    ///
    /// # Errors
    /// Over 1 KiB, or it does not compile within the size limit; why.
    pub fn new(source: &str) -> Result<Self, String> {
        if source.len() > MAX_PATTERN {
            return Err(format!("a pattern is at most {MAX_PATTERN} bytes"));
        }
        let regex = RegexBuilder::new(&format!("^(?:{source})$"))
            .size_limit(PATTERN_SIZE)
            .build()
            .map_err(|e| format!("does not compile: {e}"))?;
        Ok(Self {
            source: source.to_string(),
            regex,
        })
    }

    /// Whether `text` matches it whole.
    #[must_use]
    pub fn matches(&self, text: &str) -> bool {
        self.regex.is_match(text)
    }
}

/// A rule or a constraint a value broke.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    /// Where (`items[3].email`); empty: the value itself.
    pub path: String,
    /// What: a constraint's name or a rule's.
    pub rule: String,
    /// What is wrong, for a person.
    pub message: String,
}

impl Constraints {
    /// Whether they bound nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.minimum.is_none()
            && self.maximum.is_none()
            && self.min_length.is_none()
            && self.max_length.is_none()
            && self.pattern.is_none()
            && self.one_of.is_empty()
    }

    /// That they fit a field of `ty`: bounds of its type and one that
    /// orders, lengths of one that has a length, a pattern of text, values
    /// of its type.
    ///
    /// # Errors
    /// The part at fault (`minimum`) and why.
    pub fn fit(&self, ty: &Type) -> Result<(), (&'static str, String)> {
        for (part, bound) in [("minimum", &self.minimum), ("maximum", &self.maximum)] {
            if let Some(bound) = bound {
                if !ordered(ty) {
                    return Err((part, format!("a {ty} has no order")));
                }
                if !ty.accepts(&bound.value.ty()) {
                    return Err((part, format!("is a {}, not a {ty}", bound.value.ty())));
                }
            }
        }
        if let (Some(min), Some(max)) = (&self.minimum, &self.maximum)
            && compare(&min.value, &max.value) == Some(Ordering::Greater)
        {
            return Err(("minimum", "is above the maximum".to_string()));
        }
        let lengthy = matches!(
            ty,
            Type::String | Type::Bytes | Type::List(_) | Type::Map(_)
        );
        for (part, length) in [
            ("min_length", self.min_length),
            ("max_length", self.max_length),
        ] {
            if length.is_some() && !lengthy {
                return Err((part, format!("a {ty} has no length")));
            }
        }
        if let (Some(min), Some(max)) = (self.min_length, self.max_length)
            && min > max
        {
            return Err(("min_length", "is above the max_length".to_string()));
        }
        if self.pattern.is_some() && *ty != Type::String {
            return Err(("pattern", format!("a {ty} is not text")));
        }
        if let Some(other) = self.one_of.iter().find(|v| !ty.accepts(&v.value.ty())) {
            return Err(("one_of", format!("has a {}, not a {ty}", other.value.ty())));
        }
        Ok(())
    }

    /// Every constraint `value` breaks, at `path`.
    pub fn broken(&self, value: &Value, path: &str, out: &mut Vec<Violation>) {
        let mut broke = |rule: &str, message: String| {
            out.push(Violation {
                path: path.to_string(),
                rule: rule.to_string(),
                message,
            });
        };
        if let Some(min) = &self.minimum
            && compare(value, &min.value) == Some(Ordering::Less)
        {
            broke("minimum", format!("is below {}", min.json));
        }
        if let Some(max) = &self.maximum
            && compare(value, &max.value) == Some(Ordering::Greater)
        {
            broke("maximum", format!("is above {}", max.json));
        }
        if let Some(length) = length(value) {
            if let Some(min) = self.min_length
                && length < min
            {
                broke("min_length", format!("has {length}, fewer than {min}"));
            }
            if let Some(max) = self.max_length
                && length > max
            {
                broke("max_length", format!("has {length}, more than {max}"));
            }
        }
        if let (Some(pattern), Value::String(text)) = (&self.pattern, value)
            && !pattern.matches(text)
        {
            broke(
                "pattern",
                format!("\"{text}\" does not match {}", pattern.source),
            );
        }
        if !self.one_of.is_empty() && !self.one_of.iter().any(|b| b.value == *value) {
            let all: Vec<String> = self.one_of.iter().map(|b| b.json.to_string()).collect();
            broke("one_of", format!("is none of {}", all.join(", ")));
        }
    }
}

/// Every field of `fields` that `value` (a map or a message) breaks, at
/// `path`: a required one missing, one of another type, its constraints.
pub fn violations(fields: &[Field], value: &Value, path: &str, out: &mut Vec<Violation>) {
    Checker::new(fields.to_vec()).violations(value, path, out);
}

/// A schema's fields, ready to check many values.
#[derive(Debug, Clone)]
pub struct Checker {
    fields: Vec<Field>,
}

impl Checker {
    /// The checker of `fields`.
    #[must_use]
    pub fn new(fields: Vec<Field>) -> Self {
        Self { fields }
    }

    /// Whether it has no fields.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    /// Every field `value` (a map or a message) breaks, at `path`, into
    /// `out`.
    pub fn violations(&self, value: &Value, path: &str, out: &mut Vec<Violation>) {
        match value {
            Value::Map(entries) => of_fields(&self.fields, |name| entries.get(name), path, out),
            Value::Message(m) => of_fields(&self.fields, |name| message_field(m, name), path, out),
            other => out.push(Violation {
                path: path.to_string(),
                rule: "type".to_string(),
                message: format!("is a {}, not a map or a message", other.ty()),
            }),
        }
    }
}

/// Every field of `fields` broken, each read by `get` (none: not given),
/// at `path`.
pub fn of_fields(
    fields: &[Field],
    get: impl Fn(&str) -> Result<Option<Value>, String>,
    path: &str,
    out: &mut Vec<Violation>,
) {
    let at = |name: &str| {
        if path.is_empty() {
            name.to_string()
        } else {
            format!("{path}.{name}")
        }
    };
    for field in fields {
        let mut broke = |rule: &str, message: String| {
            out.push(Violation {
                path: at(&field.name),
                rule: rule.to_string(),
                message,
            });
        };
        match get(&field.name) {
            Err(why) => broke("type", why),
            Ok(None | Some(Value::Null)) if field.required => {
                broke("required", "is required".to_string());
            }
            Ok(None | Some(Value::Null)) => {}
            Ok(Some(given)) if !field.ty.accepts(&given.ty()) => {
                broke("type", format!("is a {}, not a {}", given.ty(), field.ty));
            }
            Ok(Some(given)) => {
                if !field.constraints.is_empty() {
                    // The path is written only for what broke.
                    let before = out.len();
                    field.constraints.broken(&given, "", out);
                    for v in &mut out[before..] {
                        v.path = at(&field.name);
                    }
                }
            }
        }
    }
}

/// Field `name` of `m`: none when it is not set.
///
/// # Errors
/// `m` has no such field, or its value is none here; why.
fn message_field(m: &DynamicMessage, name: &str) -> Result<Option<Value>, String> {
    let field = m
        .descriptor()
        .get_field_by_name(name)
        .ok_or_else(|| format!("a {} has no field {name}", m.descriptor().full_name()))?;
    if !m.has_field(&field) {
        return Ok(None);
    }
    Value::of_proto(&field.kind(), &m.get_field(&field)).map(Some)
}

/// Whether values of `ty` are ordered.
fn ordered(ty: &Type) -> bool {
    matches!(
        ty,
        Type::Int
            | Type::Uint
            | Type::Double
            | Type::String
            | Type::Timestamp
            | Type::Duration
            | Type::Dyn
    )
}

/// How `a` and `b` compare: numbers of every kind with each other, text,
/// times, durations; none for others.
fn compare(a: &Value, b: &Value) -> Option<Ordering> {
    let number = |v: &Value| match v {
        Value::Int(i) => Some(*i as f64),
        Value::Uint(u) => Some(*u as f64),
        Value::Double(d) => Some(*d),
        _ => None,
    };
    match (a, b) {
        (Value::Int(a), Value::Int(b)) => Some(a.cmp(b)),
        (Value::Uint(a), Value::Uint(b)) => Some(a.cmp(b)),
        (Value::String(a), Value::String(b)) => Some(a.cmp(b)),
        (Value::Timestamp(a), Value::Timestamp(b)) => Some(a.cmp(b)),
        (Value::Duration(a), Value::Duration(b)) => Some(a.cmp(b)),
        _ => number(a)?.partial_cmp(&number(b)?),
    }
}

/// The length of `value`: characters of text, bytes, items, entries; none
/// for others.
fn length(value: &Value) -> Option<u64> {
    let n = match value {
        Value::String(s) => s.chars().count(),
        Value::Bytes(b) => b.len(),
        Value::List(items) => items.len(),
        Value::Map(entries) => entries.len(),
        _ => return None,
    };
    // A length in memory fits a u64.
    Some(n as u64)
}

#[cfg(test)]
mod tests;
