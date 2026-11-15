// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A dry run's ports (docs/modules/workflow.md, triggers and approval):
//! nothing it does is stored. Its runs and journal are documents held in
//! memory; its workflows are the kept ones with the one it runs over them;
//! its store and blobs read the real ones under what it wrote itself.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use pcx_documents::{Documents, InMemoryDocuments};
use pcx_module_api::{ModuleError, ModuleErrorKind};
use post_cortex_proto::workflow::{StateEntry, Workflow};
use sha2::{Digest, Sha256};

use crate::adapters::runs::DocumentRuns;
use crate::ports::{Blobs, Dry, DryPorts, Kept, State, Workflows};

/// The runs and journal of a dry run, in memory.
fn runs() -> Result<DocumentRuns, ModuleError> {
    let docs: Arc<dyn Documents> = Arc::new(InMemoryDocuments::new(
        Some(crate::domain::MODULE_ID),
        post_cortex_proto::WORKFLOW_DESCRIPTOR_SET,
    )?);
    Ok(DocumentRuns::new(docs))
}

/// The kept stores a dry run reads under its own writes.
pub struct DryStores {
    /// The kept workflows.
    pub workflows: Arc<dyn Workflows>,
    /// The kept stores.
    pub state: Arc<dyn State>,
    /// Memory's blobs.
    pub blobs: Arc<dyn Blobs>,
}

impl Dry for DryStores {
    fn ports(&self, workflow: Kept<Workflow>) -> Result<DryPorts, ModuleError> {
        Ok(DryPorts {
            workflows: Arc::new(DryWorkflows::new(Arc::clone(&self.workflows), workflow)),
            runs: Arc::new(runs()?),
            state: Arc::new(DryState::new(Arc::clone(&self.state), runs()?)),
            blobs: Arc::new(DryBlobs::new(Arc::clone(&self.blobs))),
        })
    }
}

/// The kept workflows with `this` over them: the one a dry run runs.
struct DryWorkflows {
    kept: Arc<dyn Workflows>,
    this: Kept<Workflow>,
}

impl DryWorkflows {
    /// `this` over `kept`.
    fn new(kept: Arc<dyn Workflows>, this: Kept<Workflow>) -> Self {
        Self { kept, this }
    }
}

fn written() -> ModuleError {
    ModuleError::new(
        ModuleErrorKind::FailedPrecondition,
        "a dry run writes no workflow",
    )
}

#[async_trait]
impl Workflows for DryWorkflows {
    async fn named(&self, name: &str) -> Result<Option<Kept<Workflow>>, ModuleError> {
        if name == self.this.data.name {
            return Ok(Some(self.this.clone()));
        }
        self.kept.named(name).await
    }

    async fn get(&self, id: &str) -> Result<Option<Kept<Workflow>>, ModuleError> {
        if id == self.this.id {
            return Ok(Some(self.this.clone()));
        }
        self.kept.get(id).await
    }

    async fn at(&self, id: &str, version: u64) -> Result<Option<Kept<Workflow>>, ModuleError> {
        if id == self.this.id && version == self.this.version {
            return Ok(Some(self.this.clone()));
        }
        self.kept.at(id, version).await
    }

    async fn all(&self) -> Result<Vec<Kept<Workflow>>, ModuleError> {
        let mut all = self.kept.all().await?;
        all.retain(|w| w.data.name != self.this.data.name);
        all.push(self.this.clone());
        Ok(all)
    }

    async fn create(&self, _workflow: &Workflow) -> Result<Kept<Workflow>, ModuleError> {
        Err(written())
    }

    async fn change(
        &self,
        _id: &str,
        _version: u64,
        _workflow: &Workflow,
    ) -> Result<Kept<Workflow>, ModuleError> {
        Err(written())
    }

    async fn delete(&self, _id: &str) -> Result<(), ModuleError> {
        Err(written())
    }
}

/// The real stores under what a dry run wrote: a key it put or deleted
/// reads as it left it; another reads as kept.
struct DryState {
    kept: Arc<dyn State>,
    mine: DocumentRuns,
    /// The entries it deleted, by id.
    deleted: Mutex<HashSet<String>>,
}

impl DryState {
    /// Over `kept`, writing into `mine`.
    fn new(kept: Arc<dyn State>, mine: DocumentRuns) -> Self {
        Self {
            kept,
            mine,
            deleted: Mutex::default(),
        }
    }
}

#[async_trait]
impl State for DryState {
    async fn get(
        &self,
        workflow: &str,
        key: &str,
    ) -> Result<Option<Kept<StateEntry>>, ModuleError> {
        if let Some(mine) = self.mine.get(workflow, key).await? {
            return Ok(Some(mine));
        }
        match self.kept.get(workflow, key).await? {
            Some(kept) if self.deleted.lock().contains(&kept.id) => Ok(None),
            found => Ok(found),
        }
    }

    /// Written over its own copy, whatever was read.
    async fn put(&self, entry: &StateEntry, _read: Option<(&str, u64)>) -> Result<(), ModuleError> {
        let mine = self.mine.get(&entry.workflow, &entry.key).await?;
        self.mine
            .put(entry, mine.as_ref().map(|m| (m.id.as_str(), m.version)))
            .await
    }

    async fn delete(&self, id: &str) -> Result<(), ModuleError> {
        match self.mine.delete(id).await {
            Err(e) if e.kind != ModuleErrorKind::NotFound => return Err(e),
            _ => {}
        }
        self.deleted.lock().insert(id.to_string());
        Ok(())
    }
}

/// The real blobs under the ones a dry run put, held in memory.
struct DryBlobs {
    kept: Arc<dyn Blobs>,
    mine: Mutex<HashMap<String, Vec<u8>>>,
}

impl DryBlobs {
    /// Over `kept`.
    fn new(kept: Arc<dyn Blobs>) -> Self {
        Self {
            kept,
            mine: Mutex::default(),
        }
    }
}

#[async_trait]
impl Blobs for DryBlobs {
    async fn put(&self, bytes: Vec<u8>) -> Result<String, ModuleError> {
        let sha256 = format!("{:x}", Sha256::digest(&bytes));
        self.mine.lock().insert(sha256.clone(), bytes);
        Ok(sha256)
    }

    async fn get(&self, sha256: &str) -> Result<Vec<u8>, ModuleError> {
        let mine = self.mine.lock().get(sha256).cloned();
        match mine {
            Some(bytes) => Ok(bytes),
            None => self.kept.get(sha256).await,
        }
    }
}
