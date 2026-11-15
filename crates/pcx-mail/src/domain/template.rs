// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A mail template's rules (docs/modules/mail.md, templates): its name,
//! its inputs with their types and examples, its subject and bodies
//! compiled against the inputs when it is saved (`pcx-expr`), and filled
//! when it is used, the HTML body's values escaped.

use pcx_expr::{Escape, Inputs, Template as Compiled, Type, Value};
use post_cortex_proto::mail::{Filled, Template};
use serde_json::{Map, Value as Json};

use crate::domain::{Broken, MODULE_ID};

/// The kind of its templates' documents in the module.
pub const TEMPLATE_KIND: &str = "template";

/// Its templates' kind as memory keeps it, `mail/template`.
#[must_use]
pub fn template_kind() -> String {
    format!("{MODULE_ID}/{TEMPLATE_KIND}")
}

/// A template, checked and compiled.
pub struct Checked {
    inputs: Inputs,
    /// Its inputs' examples, by name.
    examples: Map<String, Json>,
    subject: Compiled,
    html: Option<Compiled>,
    text: Compiled,
}

/// `template` checked: its name 1 to 64 of a-z, 0-9 and '-'; each input
/// named as CEL names a variable, of a type CEL writes, with an example of
/// it; a subject and a text body; every expression of the subject and the
/// bodies checked against the inputs.
///
/// # Errors
/// The first rule it breaks, naming the field (`inputs[1].type`,
/// `subject`) and, in an expression, its offset.
pub fn check(template: &Template) -> Result<Checked, Broken> {
    let name = &template.name;
    if !pcx_module_api::name::is_slug(name) {
        return Err(Broken::new(
            "name",
            format!(
                "has \"{name}\": 1 to {} of a-z, 0-9 and -",
                pcx_module_api::name::NAME_CHARS
            ),
        ));
    }
    let mut declared = Vec::with_capacity(template.inputs.len());
    let mut examples = Map::new();
    for (i, input) in template.inputs.iter().enumerate() {
        let ty = Type::parse(&input.r#type)
            .map_err(|why| Broken::new(&format!("inputs[{i}].type"), why))?;
        let example = input
            .example
            .as_ref()
            .map(json)
            .transpose()
            .map_err(|why| Broken::new(&format!("inputs[{i}].example"), why))?
            .ok_or_else(|| {
                Broken::new(
                    &format!("inputs[{i}].example"),
                    "is required: a preview and a test fill the template with it",
                )
            })?;
        Value::read(&ty, &example)
            .map_err(|why| Broken::new(&format!("inputs[{i}].example"), why))?;
        examples.insert(input.name.clone(), example);
        declared.push((input.name.clone(), ty));
    }
    let inputs = Inputs::new(declared).map_err(|e| Broken::new("inputs", e.to_string()))?;
    if template.subject.trim().is_empty() {
        return Err(Broken::new("subject", "is required"));
    }
    if template.text.trim().is_empty() {
        return Err(Broken::new(
            "text",
            "is required: every message has a text body",
        ));
    }
    let compile = |field: &str, source: &str| {
        Compiled::compile(source, &inputs).map_err(|e| Broken::new(field, e.to_string()))
    };
    let subject = compile("subject", &template.subject)?;
    let html = if template.html.is_empty() {
        None
    } else {
        Some(compile("html", &template.html)?)
    };
    let text = compile("text", &template.text)?;
    Ok(Checked {
        inputs,
        examples,
        subject,
        html,
        text,
    })
}

/// A JSON value of the wire's (`google.protobuf.Value`).
///
/// # Errors
/// It does not encode as JSON (a number that is not finite).
pub fn json(value: &impl serde::Serialize) -> Result<Json, String> {
    serde_json::to_value(value).map_err(|e| format!("is not JSON: {e}"))
}

impl Checked {
    /// The template filled with `values` (none: its inputs' examples): the
    /// subject and the text as they are, the HTML body's values escaped.
    ///
    /// # Errors
    /// `values` breaks the inputs (field `values.<name>`), or an expression
    /// fails (its part and offset).
    pub fn fill(&self, values: Option<&Map<String, Json>>) -> Result<Filled, Broken> {
        let given = values.unwrap_or(&self.examples);
        let values = self
            .inputs
            .read(given)
            .map_err(|e| Broken::new(&format!("values.{}", e.at), e.message))?;
        let fill = |field: &str, template: &Compiled, escape: Escape| {
            template
                .fill(&values, escape)
                .map_err(|e| Broken::new(field, e.to_string()))
        };
        Ok(Filled {
            subject: fill("subject", &self.subject, Escape::None)?,
            html: match &self.html {
                Some(html) => fill("html", html, Escape::Html)?,
                None => String::new(),
            },
            text: fill("text", &self.text, Escape::None)?,
        })
    }
}

#[cfg(test)]
mod tests;
