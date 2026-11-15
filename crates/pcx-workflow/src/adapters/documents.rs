// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The workflows as memory's documents (docs/modules/workflow.md, why
//! documents): kind `workflow/workflow`, titled and searched by name and
//! description, each change the document's next version.

use std::sync::Arc;

use async_trait::async_trait;
use pcx_documents::{Documents, Item, Typed};
use pcx_module_api::{ModuleError, ModuleErrorKind};
use post_cortex_proto::workflow::Workflow;

use crate::domain::{self, WORKFLOW_KIND, kind};
use crate::ports::{Kept, Workflows};

/// A document as the ports keep it.
pub fn kept<T>(item: Item<T>) -> Kept<T> {
    Kept {
        id: item.document.id,
        version: item.document.version,
        data: item.data,
    }
}

/// The workflows over `pcx-documents`.
pub struct DocumentWorkflows {
    docs: Arc<dyn Documents>,
}

impl DocumentWorkflows {
    /// The workflows of `docs`.
    pub fn new(docs: Arc<dyn Documents>) -> Self {
        Self { docs }
    }

    fn typed(&self) -> Typed<'_> {
        Typed::new(self.docs.as_ref(), kind(WORKFLOW_KIND))
    }
}

#[async_trait]
impl Workflows for DocumentWorkflows {
    async fn named(&self, name: &str) -> Result<Option<Kept<Workflow>>, ModuleError> {
        let example = Workflow {
            name: name.to_string(),
            ..Default::default()
        };
        let mut found = self.typed().all(Some((&example, &["name"][..]))).await?;
        match found.len() {
            0 => Ok(None),
            1 => Ok(found.pop().map(kept)),
            n => Err(ModuleError::new(
                ModuleErrorKind::Conflict,
                format!("{n} workflows are named {name}; rename all but one"),
            )),
        }
    }

    async fn get(&self, id: &str) -> Result<Option<Kept<Workflow>>, ModuleError> {
        Ok(self.typed().find(id).await?.map(kept))
    }

    async fn at(&self, id: &str, version: u64) -> Result<Option<Kept<Workflow>>, ModuleError> {
        Ok(self.typed().find_version(id, version).await?.map(kept))
    }

    async fn all(&self) -> Result<Vec<Kept<Workflow>>, ModuleError> {
        Ok(self
            .typed()
            .all::<Workflow>(None)
            .await?
            .into_iter()
            .map(kept)
            .collect())
    }

    async fn create(&self, workflow: &Workflow) -> Result<Kept<Workflow>, ModuleError> {
        self.typed()
            .create(workflow, &domain::text(workflow))
            .await
            .map(kept)
    }

    async fn change(
        &self,
        id: &str,
        version: u64,
        workflow: &Workflow,
    ) -> Result<Kept<Workflow>, ModuleError> {
        self.typed()
            .change(id, version, workflow, &domain::text(workflow))
            .await
            .map(kept)
    }

    async fn delete(&self, id: &str) -> Result<(), ModuleError> {
        self.typed().delete(id).await
    }
}
