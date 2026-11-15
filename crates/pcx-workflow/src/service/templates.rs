// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The workflow templates the modules ship (docs/modules/workflow.md,
//! templates): listed to an author who reaches their module, each with the
//! workflow made from it when one is kept, and made a workflow with the
//! author's values (the engine keeps it as any new workflow).

use std::collections::HashMap;

use pcx_module_api::{ModuleError, ModuleErrorKind, Scope};
use post_cortex_proto::workflow::{TemplateEntry, Workflow};

use crate::domain::template;
use crate::service::{Service, refused};

impl Service<'_> {
    /// The templates of the modules `scope` reaches, those of `module`
    /// only when it names one, by module and name.
    ///
    /// # Errors
    /// The host or memory failed.
    pub async fn templates(
        &self,
        scope: &Scope,
        module: &str,
    ) -> Result<Vec<TemplateEntry>, ModuleError> {
        let mut shipped = self.tools.templates(scope).await?;
        shipped.retain(|(of, _)| module.is_empty() || of == module);
        shipped.sort_by(|a, b| (&a.0, &a.1.name).cmp(&(&b.0, &b.1.name)));
        let mut listed = Vec::with_capacity(shipped.len());
        for (module, template) in shipped {
            let installed = self
                .workflows
                .named(&template.name)
                .await?
                .map(|kept| kept.data.name)
                .unwrap_or_default();
            listed.push(TemplateEntry {
                module,
                template: Some(template),
                installed,
            });
        }
        Ok(listed)
    }

    /// The workflow template `name` of `module` makes with `values`, for
    /// `scope`'s author; not kept.
    ///
    /// # Errors
    /// `NotFound` when no module `scope` reaches ships it;
    /// `InvalidArgument` for the values or a template that does not read.
    pub async fn template(
        &self,
        scope: &Scope,
        module: &str,
        name: &str,
        values: &HashMap<String, String>,
    ) -> Result<Workflow, ModuleError> {
        let (_, shipped) = self
            .tools
            .templates(scope)
            .await?
            .into_iter()
            .find(|(of, template)| of == module && template.name == name)
            .ok_or_else(|| {
                ModuleError::new(
                    ModuleErrorKind::NotFound,
                    format!("no module {module} this author reaches ships a template {name}"),
                )
                .with_field("template")
            })?;
        template::installed(&shipped, values).map_err(|r| refused(&r))
    }
}
