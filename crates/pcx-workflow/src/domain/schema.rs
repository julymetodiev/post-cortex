// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A `Schema` read: its fields (`pcx_expr::schema`), each with an example
//! of its type when it has one, and a run's input read by them.

use std::collections::BTreeMap;

use pcx_expr::constraints::{Bound, Constraints, Pattern};
use pcx_expr::schema::{Field, bounds, zero};
use pcx_expr::{Type, Value};
use post_cortex_proto::workflow::{Map, Schema};
use prost_reflect::DescriptorPool;

use crate::domain::value::{read, type_of};

/// The fields of `schema` (none: no fields).
///
/// # Errors
/// The field at fault (`fields[1].type`) and why.
pub fn fields(
    schema: Option<&Schema>,
    messages: &DescriptorPool,
) -> Result<Vec<Field>, (String, String)> {
    let Some(schema) = schema else {
        return Ok(Vec::new());
    };
    let mut fields = pcx_expr::schema::fields(
        schema
            .fields
            .iter()
            .map(|f| (f.name.as_str(), f.r#type.as_str(), f.required)),
    )
    .map_err(|(i, part, why)| (format!("fields[{i}].{part}"), why))?;
    for (i, (field, read_field)) in schema.fields.iter().zip(&mut fields).enumerate() {
        if let Some(given) = &field.constraints {
            read_field.constraints = constraints(given, &read_field.ty, messages)
                .map_err(|(part, why)| (format!("fields[{i}].constraints.{part}"), why))?;
        }
        let Some(example) = &field.example else {
            continue;
        };
        let at = || format!("fields[{i}].example");
        let given = type_of(example, messages).map_err(|why| (at(), why))?;
        if !read_field.ty.accepts(&given) {
            return Err((at(), format!("is a {given}, not a {}", read_field.ty)));
        }
        read_field.example = Some(read(example, messages).map_err(|why| (at(), why))?);
    }
    Ok(fields)
}

/// `given`, a field's constraints, read for a field of `ty`.
///
/// # Errors
/// The part at fault (`minimum`) and why.
fn constraints(
    given: &post_cortex_proto::workflow::Constraints,
    ty: &Type,
    messages: &DescriptorPool,
) -> Result<Constraints, (String, String)> {
    let bound = |part: &str, v: &post_cortex_proto::workflow::Value| {
        read(v, messages)
            .and_then(Bound::new)
            .map_err(|why| (part.to_string(), why))
    };
    let read = Constraints {
        minimum: given
            .minimum
            .as_ref()
            .map(|v| bound("minimum", v))
            .transpose()?,
        maximum: given
            .maximum
            .as_ref()
            .map(|v| bound("maximum", v))
            .transpose()?,
        min_length: given.min_length,
        max_length: given.max_length,
        pattern: if given.pattern.is_empty() {
            None
        } else {
            Some(Pattern::new(&given.pattern).map_err(|why| ("pattern".to_string(), why))?)
        },
        one_of: given
            .one_of
            .iter()
            .enumerate()
            .map(|(n, v)| bound(&format!("one_of[{n}]"), v))
            .collect::<Result<_, _>>()?,
    };
    read.fit(ty)
        .map_err(|(part, why)| (part.to_string(), why))?;
    Ok(read)
}

/// `given`, a run's input, read by `fields` as
/// [`pcx_expr::schema::read_answer`] reads an answer.
///
/// # Errors
/// As [`pcx_expr::schema::read_answer`]'s; a value that does not read.
pub fn read_input(
    fields: &[Field],
    given: &Map,
    messages: &DescriptorPool,
) -> Result<BTreeMap<String, Value>, String> {
    if let Some(extra) = given
        .entries
        .keys()
        .find(|k| !fields.iter().any(|f| &f.name == *k))
    {
        return Err(format!("{extra} is no field of the input"));
    }
    fields
        .iter()
        .map(|f| {
            let value = match given.entries.get(&f.name) {
                Some(value) => {
                    let value =
                        read(value, messages).map_err(|why| format!("{}: {why}", f.name))?;
                    if !f.ty.accepts(&value.ty()) {
                        return Err(format!("{}: a {} is not a {}", f.name, value.ty(), f.ty));
                    }
                    value
                }
                None if f.required => return Err(format!("{} is required", f.name)),
                None => zero(&f.ty),
            };
            Ok((f.name.clone(), value))
        })
        .collect::<Result<_, String>>()
        .and_then(|read| bounds(fields, read))
}
