// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The fields of a schema (a workflow's input, a `model` or `agent` block's
//! output, docs/modules/workflow.md; a request's `expect`,
//! docs/modules/http.md), each named as CEL names a variable and of a type
//! CEL writes; their zero values, the JSON Schema a model answers by, and
//! JSON read by them.

use std::collections::BTreeMap;

use serde_json::{Value as Json, json};

use crate::constraints::{Constraints, of_fields};
use crate::inputs::Inputs;
use crate::types::Type;
use crate::value::Value;

/// A field of a schema, read.
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    /// Its name.
    pub name: String,
    /// Its type.
    pub ty: Type,
    /// Whether a value must be given.
    pub required: bool,
    /// A value of it, for a dry run; none: its zero value.
    pub example: Option<Value>,
    /// What its values must be beside their type.
    pub constraints: Constraints,
}

/// The fields `given` (each its name, its type as CEL writes it and
/// whether it is required), without examples.
///
/// # Errors
/// The index of the field at fault, the part (`name`, `type`) and why: a
/// name that is no variable's or is given twice, a type that does not
/// parse.
pub fn fields<'a>(
    given: impl IntoIterator<Item = (&'a str, &'a str, bool)>,
) -> Result<Vec<Field>, (usize, &'static str, String)> {
    let mut fields: Vec<Field> = Vec::new();
    for (i, (name, ty, required)) in given.into_iter().enumerate() {
        // A name is a variable's: the rules are the inputs'.
        Inputs::new([(name.to_string(), Type::Dyn)]).map_err(|e| (i, "name", e.message))?;
        if fields.iter().any(|f| f.name == name) {
            return Err((i, "name", format!("{name} is a field already")));
        }
        let ty = Type::parse(ty).map_err(|why| (i, "type", why))?;
        fields.push(Field {
            name: name.to_string(),
            ty,
            required,
            example: None,
            constraints: Constraints::default(),
        });
    }
    Ok(fields)
}

/// The zero value of `ty`: what a field not given is.
pub fn zero(ty: &Type) -> Value {
    match ty {
        Type::String => Value::string(""),
        Type::Int => Value::Int(0),
        Type::Uint => Value::Uint(0),
        Type::Double => Value::Double(0.0),
        Type::Bool => Value::Bool(false),
        Type::Bytes => Value::Bytes(Vec::new().into()),
        Type::Timestamp => Value::Timestamp(chrono::DateTime::UNIX_EPOCH),
        Type::Duration => Value::Duration(chrono::TimeDelta::zero()),
        Type::List(_) => Value::list([]),
        Type::Map(_) => Value::map([]),
        Type::Message(_) | Type::Null | Type::Dyn => Value::Null,
    }
}

/// The JSON Schema of an object of `fields`: what a model's forced tool
/// takes.
pub fn json_schema(fields: &[Field]) -> Json {
    fn of(ty: &Type) -> Json {
        match ty {
            Type::String => json!({ "type": "string" }),
            Type::Int | Type::Uint => json!({ "type": "integer" }),
            Type::Double => json!({ "type": "number" }),
            Type::Bool => json!({ "type": "boolean" }),
            Type::Bytes => json!({ "type": "string", "contentEncoding": "base64" }),
            Type::Timestamp => json!({ "type": "string", "format": "date-time" }),
            Type::Duration => json!({ "type": "string", "pattern": "^-?[0-9]+(\\.[0-9]{1,9})?s$" }),
            Type::List(item) => json!({ "type": "array", "items": of(item) }),
            Type::Map(value) => json!({ "type": "object", "additionalProperties": of(value) }),
            Type::Message(_) | Type::Null | Type::Dyn => json!({}),
        }
    }
    let properties: serde_json::Map<String, Json> = fields
        .iter()
        .map(|f| {
            let mut schema = of(&f.ty);
            bounded(&mut schema, &f.ty, &f.constraints);
            (f.name.clone(), schema)
        })
        .collect();
    let required: Vec<&str> = fields
        .iter()
        .filter(|f| f.required)
        .map(|f| f.name.as_str())
        .collect();
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

/// `schema`, a field's of `ty`, with the bounds a model is held to: a
/// number's `minimum` and `maximum`, text's lengths and `pattern`, a list's
/// items, an object's properties, and `enum`.
fn bounded(schema: &mut Json, ty: &Type, constraints: &Constraints) {
    let Json::Object(schema) = schema else {
        return;
    };
    let numeric = matches!(ty, Type::Int | Type::Uint | Type::Double);
    for (key, bound) in [
        ("minimum", &constraints.minimum),
        ("maximum", &constraints.maximum),
    ] {
        if let (true, Some(bound)) = (numeric, bound) {
            schema.insert(key.into(), bound.json().clone());
        }
    }
    let (min, max) = match ty {
        Type::String => ("minLength", "maxLength"),
        Type::List(_) => ("minItems", "maxItems"),
        Type::Map(_) => ("minProperties", "maxProperties"),
        _ => ("", ""),
    };
    for (key, length) in [(min, constraints.min_length), (max, constraints.max_length)] {
        if let (false, Some(length)) = (key.is_empty(), length) {
            schema.insert(key.into(), json!(length));
        }
    }
    if let Some(pattern) = &constraints.pattern {
        schema.insert(
            "pattern".into(),
            json!(format!("^(?:{})$", pattern.source())),
        );
    }
    if !constraints.one_of.is_empty() {
        schema.insert(
            "enum".into(),
            Json::Array(
                constraints
                    .one_of
                    .iter()
                    .map(|b| b.json().clone())
                    .collect(),
            ),
        );
    }
}

/// What a dry run answers for `fields`: each field's example, or its zero
/// value.
#[must_use]
pub fn example(fields: &[Field]) -> BTreeMap<String, Value> {
    fields
        .iter()
        .map(|f| {
            let value = f.example.clone().unwrap_or_else(|| zero(&f.ty));
            (f.name.clone(), value)
        })
        .collect()
}

/// `answer`, a model's or an agent's JSON, read by `fields`: each field of
/// its type, a required one given, another its zero value.
///
/// # Errors
/// It is not an object, a required field is missing, a field is of
/// another type or not a field; why.
pub fn read_answer(fields: &[Field], answer: &Json) -> Result<BTreeMap<String, Value>, String> {
    read_object(fields, answer, "answer", false)
}

/// `body`, a webhook's JSON, read by `fields` as a run's input: as
/// [`read_answer`] reads an answer, but the fields it does not name are
/// left out.
///
/// # Errors
/// It is not an object; a value that does not read, or a required one
/// missing.
pub fn read_body(fields: &[Field], body: &Json) -> Result<BTreeMap<String, Value>, String> {
    read_object(fields, body, "body", true)
}

fn read_object(
    fields: &[Field],
    json: &Json,
    what: &str,
    extras: bool,
) -> Result<BTreeMap<String, Value>, String> {
    let Json::Object(given) = json else {
        return Err(format!("the {what} is not an object: {json}"));
    };
    if !extras && let Some(extra) = given.keys().find(|k| !fields.iter().any(|f| &f.name == *k)) {
        return Err(format!("{extra} is no field of the {what}"));
    }
    let read: BTreeMap<String, Value> = fields
        .iter()
        .map(|f| {
            let value = match given.get(&f.name) {
                Some(json) => {
                    Value::read(&f.ty, json).map_err(|why| format!("{}: {why}", f.name))?
                }
                None if f.required => return Err(format!("{} is required", f.name)),
                None => zero(&f.ty),
            };
            Ok((f.name.clone(), value))
        })
        .collect::<Result<_, String>>()?;
    bounds(fields, read)
}

/// `read`, values of `fields`, when they keep every field's constraints.
///
/// # Errors
/// Every constraint broken, one after another.
pub fn bounds(
    fields: &[Field],
    read: BTreeMap<String, Value>,
) -> Result<BTreeMap<String, Value>, String> {
    let mut broken = Vec::new();
    of_fields(fields, |name| Ok(read.get(name).cloned()), "", &mut broken);
    if broken.is_empty() {
        return Ok(read);
    }
    let all: Vec<String> = broken
        .iter()
        .map(|v| format!("{}: {}", v.path, v.message))
        .collect();
    Err(all.join("; "))
}
