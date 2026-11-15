// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The mail service's templates (docs/modules/mail.md, templates): kept
//! as documents, checked when they are saved, filled for a preview and for
//! a send made from one.

use pcx_documents::{Item, Typed};
use pcx_module_api::{ModuleError, ModuleErrorKind};
use post_cortex_proto::mail::{
    Filled, PreviewTemplateRequest, SaveTemplateRequest, SendRequest, Template, TemplateList,
    TemplateRecord,
};
use serde_json::{Map, Value as Json};

use crate::domain::template::{self, Checked, json, template_kind};
use crate::service::{Service, broken};

fn record(item: Item<Template>) -> TemplateRecord {
    TemplateRecord {
        id: item.document.id,
        version: item.document.version,
        template: Some(item.data),
    }
}

/// The JSON object of a call's values.
fn values(given: Option<&pbjson_types::Struct>) -> Result<Option<Map<String, Json>>, ModuleError> {
    let invalid = |why: String| {
        ModuleError::new(ModuleErrorKind::InvalidArgument, format!("values {why}"))
            .with_field("values".to_string())
    };
    given
        .map(|values| match json(values).map_err(invalid)? {
            Json::Object(map) => Ok(map),
            other => Err(invalid(format!("are {other}, not an object"))),
        })
        .transpose()
}

impl Service<'_> {
    fn templates(&self) -> Typed<'_> {
        Typed::new(self.docs, template_kind())
    }

    /// The templates, by name.
    ///
    /// # Errors
    /// Memory failed.
    pub async fn list_templates(&self) -> Result<TemplateList, ModuleError> {
        let mut templates: Vec<TemplateRecord> = self
            .templates()
            .all::<Template>(None)
            .await?
            .into_iter()
            .map(record)
            .collect();
        templates.sort_by(|a, b| {
            let name = |r: &TemplateRecord| r.template.as_ref().map(|t| t.name.clone());
            name(a).cmp(&name(b))
        });
        Ok(TemplateList { templates })
    }

    /// Template `name`.
    ///
    /// # Errors
    /// `NotFound` when there is none of that name; memory failed.
    pub async fn get_template(&self, name: &str) -> Result<TemplateRecord, ModuleError> {
        self.named(name).await.map(record)
    }

    async fn named(&self, name: &str) -> Result<Item<Template>, ModuleError> {
        let example = Template {
            name: name.to_string(),
            ..Template::default()
        };
        self.templates()
            .all(Some((&example, &["name"][..])))
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| {
                ModuleError::new(
                    ModuleErrorKind::NotFound,
                    format!("there is no mail template {name}"),
                )
            })
    }

    /// Add `save.template` (no id), or change template `save.id` from the
    /// version read, once it checks.
    ///
    /// # Errors
    /// `InvalidArgument` for a template the rules refuse; `Conflict` for a
    /// name another template has, or a template changed since its version;
    /// `NotFound` when it is gone; memory failed.
    pub async fn save_template(
        &self,
        save: SaveTemplateRequest,
    ) -> Result<TemplateRecord, ModuleError> {
        let template = save.template.unwrap_or_default();
        template::check(&template).map_err(broken)?;
        let taken = match self.named(&template.name).await {
            Ok(other) => other.document.id != save.id,
            Err(e) if e.kind == ModuleErrorKind::NotFound => false,
            Err(e) => return Err(e),
        };
        if taken {
            return Err(ModuleError::new(
                ModuleErrorKind::Conflict,
                format!("another mail template is named {}", template.name),
            )
            .with_field("name".to_string()));
        }
        let saved = if save.id.is_empty() {
            self.templates().create(&template, "").await?
        } else {
            self.templates()
                .change(&save.id, save.version, &template, "")
                .await?
        };
        Ok(record(saved))
    }

    /// A template filled, nothing sent: the one `preview.name` names, else
    /// `preview.template` as it is being edited; with `preview.values`, or
    /// its inputs' examples.
    ///
    /// # Errors
    /// `NotFound` for a name there is no template of; `InvalidArgument` for
    /// a template the rules refuse or values it refuses; memory failed.
    pub async fn preview_template(
        &self,
        preview: PreviewTemplateRequest,
    ) -> Result<Filled, ModuleError> {
        let template = if preview.name.is_empty() {
            preview.template.unwrap_or_default()
        } else {
            self.named(&preview.name).await?.data
        };
        let checked = template::check(&template).map_err(broken)?;
        let given = values(preview.values.as_ref())?;
        checked.fill(given.as_ref()).map_err(broken)
    }

    /// `message` as it is sent, and the template it is made from: when it
    /// names one, its subject and bodies are the template's filled with its
    /// values.
    ///
    /// # Errors
    /// `InvalidArgument` for values without a template, a subject or body
    /// of its own beside one, or values the template refuses; `NotFound`
    /// for a template there is none of; `FailedPrecondition` for a saved
    /// template that no longer checks; memory failed.
    pub async fn composed(
        &self,
        mut message: SendRequest,
    ) -> Result<(SendRequest, String), ModuleError> {
        let invalid = |field: &str, why: &str| {
            ModuleError::new(ModuleErrorKind::InvalidArgument, why.to_string())
                .with_field(field.to_string())
        };
        if message.template.is_empty() {
            if message.values.is_some() {
                return Err(invalid("values", "values are given with a template only"));
            }
            return Ok((message, String::new()));
        }
        if !(message.subject.is_empty() && message.text.is_empty() && message.html.is_empty()) {
            return Err(invalid(
                "template",
                "a message made from a template has no subject or body of its own",
            ));
        }
        let saved = self.named(&message.template).await?;
        let checked: Checked = template::check(&saved.data).map_err(|rule| {
            ModuleError::new(
                ModuleErrorKind::FailedPrecondition,
                format!(
                    "mail template {} no longer checks: {rule}; save it again",
                    message.template
                ),
            )
        })?;
        let given = values(message.values.as_ref())?.unwrap_or_default();
        let filled = checked.fill(Some(&given)).map_err(broken)?;
        message.subject = filled.subject;
        message.html = filled.html;
        message.text = filled.text;
        message.values = None;
        let name = std::mem::take(&mut message.template);
        Ok((message, name))
    }
}
