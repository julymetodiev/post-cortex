// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A module's template made a workflow (docs/modules/workflow.md,
//! templates): its parameters checked, each given a value (the installer's
//! or its default) that matches its pattern, put where its strings name it
//! (`${<name>}`), and the JSON read as a workflow named as the template.

use std::collections::{BTreeMap, HashMap};

use pcx_expr::constraints::Pattern;
use post_cortex_proto::module::WorkflowTemplate;
use post_cortex_proto::workflow::Workflow;
use prost::Name;
use prost_reflect::{DescriptorPool, DynamicMessage};
use serde_json::Value as Json;

use crate::domain::Refusal;

/// Characters of a parameter's name at most.
const PARAMETER_CHARS: usize = 32;

/// Whether `name` is 1 to [`PARAMETER_CHARS`] of a-z, 0-9 and `_`.
fn is_parameter(name: &str) -> bool {
    (1..=PARAMETER_CHARS).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// The values of `template`'s parameters: `given`'s, or their defaults.
///
/// # Errors
/// A parameter the template declares badly (`template.parameters.<name>`);
/// a value given for no parameter, none for one without a default, or one
/// that does not match its pattern (`values.<name>`).
fn values(
    template: &WorkflowTemplate,
    given: &HashMap<String, String>,
) -> Result<BTreeMap<String, String>, Refusal> {
    let mut values = BTreeMap::new();
    for parameter in &template.parameters {
        let declared = |message: String| {
            Refusal::workflow(format!("template.parameters.{}", parameter.name), message)
        };
        if !is_parameter(&parameter.name) {
            return Err(declared(format!(
                "is not 1 to {PARAMETER_CHARS} of a-z, 0-9 and _"
            )));
        }
        if values.contains_key(&parameter.name) {
            return Err(declared("is declared twice".to_string()));
        }
        if parameter.pattern.is_empty() {
            return Err(declared("has no pattern".to_string()));
        }
        let pattern = Pattern::new(&parameter.pattern)
            .map_err(|why| declared(format!("its pattern {why}")))?;
        if !parameter.default.is_empty() && !pattern.matches(&parameter.default) {
            return Err(declared(format!(
                "its default does not match {}",
                parameter.pattern
            )));
        }
        let field = format!("values.{}", parameter.name);
        let value = match given.get(&parameter.name) {
            Some(value) => value.clone(),
            None if parameter.default.is_empty() => {
                return Err(Refusal::workflow(field, "must be given: it has no default"));
            }
            None => parameter.default.clone(),
        };
        if !pattern.matches(&value) {
            return Err(Refusal::workflow(
                field,
                format!("\"{value}\" does not match {}", parameter.pattern),
            ));
        }
        values.insert(parameter.name.clone(), value);
    }
    if let Some(unknown) = given.keys().find(|name| !values.contains_key(*name)) {
        return Err(Refusal::workflow(
            format!("values.{unknown}"),
            "the template has no such parameter",
        ));
    }
    Ok(values)
}

/// `text` with each `${<name>}` replaced by its value.
///
/// # Errors
/// A `${` not closed, or a name no parameter has (`template.workflow`).
fn put(text: &str, values: &BTreeMap<String, String>) -> Result<String, Refusal> {
    let refused = |message: String| Refusal::workflow("template.workflow", message);
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("${") {
        out.push_str(&rest[..at]);
        let after = &rest[at + 2..];
        let end = after
            .find('}')
            .ok_or_else(|| refused(format!("a ${{ is not closed in \"{text}\"")))?;
        let name = &after[..end];
        let value = values
            .get(name)
            .ok_or_else(|| refused(format!("names ${{{name}}}, and no parameter is {name}")))?;
        out.push_str(value);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// `json` with the values put in each of its strings (an object's keys
/// stay as they are).
fn put_all(json: Json, values: &BTreeMap<String, String>) -> Result<Json, Refusal> {
    Ok(match json {
        Json::String(text) => Json::String(put(&text, values)?),
        Json::Array(items) => Json::Array(
            items
                .into_iter()
                .map(|item| put_all(item, values))
                .collect::<Result<_, _>>()?,
        ),
        Json::Object(fields) => Json::Object(
            fields
                .into_iter()
                .map(|(key, value)| Ok((key, put_all(value, values)?)))
                .collect::<Result<_, Refusal>>()?,
        ),
        other => other,
    })
}

/// The workflow `template` makes with `given` values, named as the
/// template. It is not checked here: it is written as any workflow is.
///
/// # Errors
/// [`values`]'s; a `${}` [`put`] refuses; a workflow that is not the JSON
/// of `pcx.workflow.v1.Workflow` (`template.workflow`).
pub fn installed(
    template: &WorkflowTemplate,
    given: &HashMap<String, String>,
) -> Result<Workflow, Refusal> {
    let values = values(template, given)?;
    let refused = |message: String| Refusal::workflow("template.workflow", message);
    let json: Json = serde_json::from_str(&template.workflow)
        .map_err(|e| refused(format!("is no JSON: {e}")))?;
    let json = put_all(json, &values)?;
    let pool = DescriptorPool::decode(post_cortex_proto::WORKFLOW_DESCRIPTOR_SET)
        .map_err(|e| refused(format!("the workflow's descriptors do not read: {e}")))?;
    let message = pool
        .get_message_by_name(&Workflow::full_name())
        .ok_or_else(|| {
            refused(format!(
                "{} is not in its descriptors",
                Workflow::full_name()
            ))
        })?;
    let mut workflow: Workflow = DynamicMessage::deserialize(message, json)
        .map_err(|e| refused(format!("is no workflow: {e}")))?
        .transcode_to()
        .map_err(|e| refused(format!("is no workflow: {e}")))?;
    workflow.name.clone_from(&template.name);
    Ok(workflow)
}

#[cfg(test)]
mod tests;
