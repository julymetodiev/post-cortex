// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The data blocks (docs/modules/workflow.md, data blocks), checked and
//! compiled once: a `filter` keeps a list's items by a condition, a
//! `transform` reshapes a value or each item into records of a schema, a
//! `validate` checks a value or each item by a schema and rules. Each is
//! pure; it runs over its variables converted once, each item binding
//! only `item` and `index` (`eval`). A record, a group and a violation
//! are checked as their messages and are, at run time, maps of their
//! fields, shared as CEL holds them: CEL reads `row.age` of a map as of a
//! message, and no message is built or copied for each item.

mod eval;
mod order;
mod waves;

pub use eval::Gave;

use std::sync::Arc;

use pcx_expr::constraints::Checker;
use pcx_expr::schema::Field;
use pcx_expr::{Expression, Inputs, Template, Type};
use post_cortex_proto::workflow::{
    FilterBlock, TransformBlock, ValidateBlock, transform_block, validate_block,
};

use crate::domain::blocks::Context;
use crate::domain::flow::{Out, Vars};
use crate::domain::keep::Keep;
use crate::domain::records;
use crate::domain::{Refusal, schema};

/// A broken rule or constraint: `steps.<id>.violations`' items.
const VIOLATION: &str = "pcx.workflow.v1.Violation";

/// An item that broke one: `steps.<id>.invalid`'s items.
const INVALID: &str = "pcx.workflow.v1.Invalid";

/// A data block, compiled.
#[derive(Debug)]
pub struct Data {
    kind: Compiled,
}

#[derive(Debug)]
enum Compiled {
    Filter(Filter),
    Transform(Transform),
    Validate(Validate),
}

#[derive(Debug)]
struct Filter {
    items: Expression,
    kept: Expression,
    distinct: Option<Expression>,
    limit: Option<usize>,
}

/// What a `transform` or a `validate` reads: one value, or a list's items.
#[derive(Debug)]
enum Input {
    Value(Expression),
    Items(Expression),
}

#[derive(Debug)]
struct Transform {
    input: Input,
    /// Each field, its key in a record, and its expression.
    fields: Vec<(Field, Arc<str>, Option<Expression>)>,
    sort_by: Option<Expression>,
    descending: bool,
    group_by: Option<Expression>,
}

#[derive(Debug)]
struct Validate {
    input: Input,
    schema: Checker,
    rules: Vec<Rule>,
}

#[derive(Debug)]
struct Rule {
    holds: Expression,
    message: Template,
    path: String,
}

/// A `filter` checked with `vars` its variables: what it gives.
///
/// # Errors
/// The first field at fault.
pub fn filter(
    context: &Context<'_>,
    keep: &mut Keep<'_>,
    vars: &Vars,
    f: &FilterBlock,
) -> Result<Out, Refusal> {
    let (items, item) = listed(keep, "filter.items", &f.items)?;
    let each = per_item(context, keep, vars, &item, "filter.items")?;
    let kept = condition(keep, "filter.where", &f.r#where, &each)?;
    let distinct = if f.distinct_by.trim().is_empty() {
        None
    } else {
        let key = compile(keep, "filter.distinct_by", &f.distinct_by, &each)?;
        if !matches!(
            key.ty(),
            Type::String | Type::Int | Type::Uint | Type::Bool | Type::Dyn
        ) {
            return Err(keep.refuse(
                "filter.distinct_by",
                format!(
                    "gives a {}, not a string, an int, a uint or a bool",
                    key.ty()
                ),
            ));
        }
        Some(key)
    };
    let limit = (f.limit > 0)
        .then(|| usize::try_from(f.limit))
        .transpose()
        .map_err(|_| keep.refuse("filter.limit", "is out of range"))?;
    insert(
        keep,
        Compiled::Filter(Filter {
            items,
            kept,
            distinct,
            limit,
        }),
    );
    Ok(Out::Fields(vec![
        ("items".into(), Type::List(Box::new(item))),
        ("dropped".into(), Type::Int),
    ]))
}

/// A `transform` checked with `vars` its variables: what it gives.
///
/// # Errors
/// The first field at fault.
pub fn transform(
    context: &Context<'_>,
    keep: &mut Keep<'_>,
    vars: &Vars,
    t: &TransformBlock,
) -> Result<Out, Refusal> {
    let fields = schema::fields(t.output.as_ref(), context.catalog.messages())
        .map_err(|(field, why)| keep.refuse(&format!("transform.output.{field}"), why))?;
    if fields.is_empty() {
        return Err(keep.refuse("transform.output", "a record has a field"));
    }
    if let Some(extra) = t
        .fields
        .keys()
        .find(|k| !fields.iter().any(|f| &f.name == *k))
    {
        return Err(keep.refuse(
            &format!("transform.fields.{extra}"),
            "is no field of the output",
        ));
    }
    let (input, each) = match &t.input {
        None => return Err(keep.refuse("transform.input", "is a value or items")),
        Some(transform_block::Input::Value(source)) => {
            let expr = compile(keep, "transform.value", source, &keep.inputs.clone())?;
            let each = inputs(
                context,
                keep,
                &vars.with("value", expr.ty().clone()),
                "transform.value",
            )?;
            (Input::Value(expr), each)
        }
        Some(transform_block::Input::Items(source)) => {
            let (expr, item) = listed(keep, "transform.items", source)?;
            let each = per_item(context, keep, vars, &item, "transform.items")?;
            (Input::Items(expr), each)
        }
    };
    let mut shaped = Vec::with_capacity(fields.len());
    for field in fields {
        let at = format!("transform.fields.{}", field.name);
        let expr = match t.fields.get(&field.name) {
            Some(source) => {
                let expr = compile(keep, &at, source, &each)?;
                if !field.ty.accepts(expr.ty()) {
                    return Err(
                        keep.refuse(&at, format!("gives a {}, not a {}", expr.ty(), field.ty))
                    );
                }
                Some(expr)
            }
            None if field.required => return Err(keep.refuse(&at, "is required")),
            None => None,
        };
        let key = Arc::from(field.name.as_str());
        shaped.push((field, key, expr));
    }
    let message = |name: String| {
        context
            .catalog
            .messages()
            .get_message_by_name(&name)
            .ok_or_else(|| {
                keep.refuse(
                    "transform.output",
                    format!("its record {name} was not made"),
                )
            })
    };
    let Input::Items(_) = input else {
        if !t.sort_by.trim().is_empty() || !t.group_by.trim().is_empty() {
            return Err(keep.refuse(
                "transform.sort_by",
                "sorts and groups a list's items: give items",
            ));
        }
        let out = Out::Fields(
            shaped
                .iter()
                .map(|(f, _, _)| (f.name.clone(), f.ty.clone()))
                .collect(),
        );
        insert(
            keep,
            Compiled::Transform(Transform {
                input,
                fields: shaped,
                sort_by: None,
                descending: false,
                group_by: None,
            }),
        );
        return Ok(out);
    };
    let record = message(records::record(context.workflow, keep.id))?;
    let group = message(records::group(context.workflow, keep.id))?;
    let rows = inputs(
        context,
        keep,
        &vars
            .with("row", Type::Message(record.full_name().to_string()))
            .with("index", Type::Int),
        "transform.sort_by",
    )?;
    let sort_by = if t.sort_by.trim().is_empty() {
        None
    } else {
        let key = compile(keep, "transform.sort_by", &t.sort_by, &rows)?;
        if !matches!(
            key.ty(),
            Type::String
                | Type::Int
                | Type::Uint
                | Type::Double
                | Type::Timestamp
                | Type::Duration
                | Type::Dyn
        ) {
            return Err(keep.refuse(
                "transform.sort_by",
                format!("gives a {}, which has no order", key.ty()),
            ));
        }
        Some(key)
    };
    let group_by = if t.group_by.trim().is_empty() {
        None
    } else {
        let key = compile(keep, "transform.group_by", &t.group_by, &rows)?;
        if !Type::String.accepts(key.ty()) {
            return Err(keep.refuse(
                "transform.group_by",
                format!("gives a {}, not a string", key.ty()),
            ));
        }
        Some(key)
    };
    let out = match group_by {
        Some(_) => ("groups", group.full_name()),
        None => ("items", record.full_name()),
    };
    let out = Out::Fields(vec![(
        out.0.to_string(),
        Type::List(Box::new(Type::Message(out.1.to_string()))),
    )]);
    insert(
        keep,
        Compiled::Transform(Transform {
            input,
            fields: shaped,
            sort_by,
            descending: t.descending,
            group_by,
        }),
    );
    Ok(out)
}

/// A `validate` checked with `vars` its variables: what it gives.
///
/// # Errors
/// The first field at fault.
pub fn validate(
    context: &Context<'_>,
    keep: &mut Keep<'_>,
    vars: &Vars,
    v: &ValidateBlock,
) -> Result<Out, Refusal> {
    let fields = schema::fields(v.schema.as_ref(), context.catalog.messages())
        .map_err(|(field, why)| keep.refuse(&format!("validate.schema.{field}"), why))?;
    if fields.is_empty() && v.rules.is_empty() {
        return Err(keep.refuse("validate", "a validate has a schema or a rule"));
    }
    let (input, read, each, item) = match &v.input {
        None => return Err(keep.refuse("validate.input", "is a value or items")),
        Some(validate_block::Input::Value(source)) => {
            let expr = compile(keep, "validate.value", source, &keep.inputs.clone())?;
            let ty = expr.ty().clone();
            let each = inputs(
                context,
                keep,
                &vars.with("value", ty.clone()),
                "validate.value",
            )?;
            (Input::Value(expr), "validate.value", each, ty)
        }
        Some(validate_block::Input::Items(source)) => {
            let (expr, item) = listed(keep, "validate.items", source)?;
            let each = per_item(context, keep, vars, &item, "validate.items")?;
            (Input::Items(expr), "validate.items", each, item)
        }
    };
    if let Type::Message(name) = &item {
        let message = context.catalog.messages().get_message_by_name(name);
        if let Some(missing) = fields.iter().find(|f| {
            message
                .as_ref()
                .is_none_or(|m| m.get_field_by_name(&f.name).is_none())
        }) {
            return Err(keep.refuse(
                "validate.schema",
                format!("{} is no field of a {name}", missing.name),
            ));
        }
    } else if !matches!(item, Type::Map(_) | Type::Dyn) && !fields.is_empty() {
        return Err(keep.refuse(
            read,
            format!("gives a {item}: a schema reads a map or a message"),
        ));
    }
    let mut rules = Vec::with_capacity(v.rules.len());
    for (n, rule) in v.rules.iter().enumerate() {
        let at = format!("validate.rules[{n}]");
        let holds = condition(keep, &format!("{at}.holds"), &rule.holds, &each)?;
        if rule.message.trim().is_empty() {
            return Err(keep.refuse(&format!("{at}.message"), "is empty"));
        }
        let message = Template::compile(&rule.message, &each)
            .map_err(|e| keep.refuse(&format!("{at}.message"), e.to_string()))?;
        rules.push(Rule {
            holds,
            message,
            path: rule.path.clone(),
        });
    }
    let message = |name: &str| {
        context
            .catalog
            .messages()
            .get_message_by_name(name)
            .ok_or_else(|| keep.refuse("validate", format!("{name} is not in the catalog")))
    };
    message(VIOLATION)?;
    message(INVALID)?;
    let violations = (
        "violations".to_string(),
        Type::List(Box::new(Type::Message(VIOLATION.into()))),
    );
    let out = match input {
        Input::Value(_) => Out::Fields(vec![violations]),
        Input::Items(_) => Out::Fields(vec![
            violations,
            ("valid".into(), Type::List(Box::new(item))),
            (
                "invalid".into(),
                Type::List(Box::new(Type::Message(INVALID.into()))),
            ),
        ]),
    };
    insert(
        keep,
        Compiled::Validate(Validate {
            input,
            schema: Checker::new(fields),
            rules,
        }),
    );
    Ok(out)
}

/// The block's compiled data kept.
fn insert(keep: &mut Keep<'_>, kind: Compiled) {
    keep.found
        .data
        .insert(keep.id.to_string(), Arc::new(Data { kind }));
}

/// `source` compiled over `inputs`, refused as `field`.
fn compile(
    keep: &Keep<'_>,
    field: &str,
    source: &str,
    inputs: &Inputs,
) -> Result<Expression, Refusal> {
    Expression::check(source, inputs).map_err(|e| keep.refuse(field, e.to_string()))
}

/// Condition `source` compiled over `inputs`: a bool.
fn condition(
    keep: &Keep<'_>,
    field: &str,
    source: &str,
    inputs: &Inputs,
) -> Result<Expression, Refusal> {
    let expr = compile(keep, field, source, inputs)?;
    if Type::Bool.accepts(expr.ty()) {
        Ok(expr)
    } else {
        Err(keep.refuse(field, format!("gives a {}, not a bool", expr.ty())))
    }
}

/// List `source` over the block's variables, and the type of its items.
fn listed(keep: &Keep<'_>, field: &str, source: &str) -> Result<(Expression, Type), Refusal> {
    let expr = compile(keep, field, source, &keep.inputs)?;
    let item = match expr.ty() {
        Type::List(item) => (**item).clone(),
        Type::Dyn => Type::Dyn,
        other => return Err(keep.refuse(field, format!("gives a {other}, not a list"))),
    };
    Ok((expr, item))
}

/// The variables of an item's expressions: the block's, `item` and `index`.
fn per_item(
    context: &Context<'_>,
    keep: &Keep<'_>,
    vars: &Vars,
    item: &Type,
    field: &str,
) -> Result<Inputs, Refusal> {
    inputs(
        context,
        keep,
        &vars.with("item", item.clone()).with("index", Type::Int),
        field,
    )
}

/// `vars` as expressions are checked against.
fn inputs(
    context: &Context<'_>,
    keep: &Keep<'_>,
    vars: &Vars,
    field: &str,
) -> Result<Inputs, Refusal> {
    vars.inputs(context.catalog)
        .map_err(|why| keep.refuse(field, why))
}

#[cfg(test)]
mod tests;
