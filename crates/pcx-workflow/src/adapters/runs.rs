// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A workflow's runs, their journals and its store as memory's documents
//! (docs/modules/workflow.md, the engine), none of them searched: kinds
//! `workflow/run`, `workflow/step` and `workflow/state`. A step's id and a
//! store entry's are worked out from what they are (UUIDv5): a step
//! written again after a crash is the same document, and two runs that
//! both add a key never both write it.

use std::sync::Arc;

use async_trait::async_trait;
use pcx_documents::{Documents, Typed};
use pcx_module_api::{ModuleError, ModuleErrorKind};
use post_cortex_proto::workflow::{Run, RunStatus, StateEntry, Step};

use crate::adapters::documents::kept;
use crate::domain::{OPEN, RUN_KIND, STATE_KIND, STEP_KIND, kind, open};
use crate::ports::{Kept, Runs, State};

/// The runs, their journals and the stores over `pcx-documents`.
pub struct DocumentRuns {
    docs: Arc<dyn Documents>,
}

impl DocumentRuns {
    /// The runs of `docs`.
    pub fn new(docs: Arc<dyn Documents>) -> Self {
        Self { docs }
    }

    fn runs(&self) -> Typed<'_> {
        Typed::new(self.docs.as_ref(), kind(RUN_KIND))
    }

    fn steps_of(&self) -> Typed<'_> {
        Typed::new(self.docs.as_ref(), kind(STEP_KIND))
    }

    fn entries(&self) -> Typed<'_> {
        Typed::new(self.docs.as_ref(), kind(STATE_KIND))
    }

    /// At most `limit` of the runs of `workflow` that `keep` keeps, the
    /// most recently changed first.
    async fn of_workflow(
        &self,
        workflow: &str,
        limit: usize,
        keep: impl Fn(&Run) -> bool + Send + Sync,
    ) -> Result<Vec<Kept<Run>>, ModuleError> {
        let example = Run {
            workflow: workflow.to_string(),
            ..Default::default()
        };
        let (runs, _) = self
            .runs()
            .after(Some((&example, &["workflow"][..])), None, limit, |item| {
                keep(&item.data)
            })
            .await?;
        Ok(runs.into_iter().map(kept).collect())
    }
}

/// The text a run's document has: its workflow and where it is.
fn run_text(run: &Run) -> String {
    let status = RunStatus::try_from(run.status).map_or("unknown", |s| s.as_str_name());
    format!("{} {status}", run.workflow)
}

#[async_trait]
impl Runs for DocumentRuns {
    async fn create(&self, run: &Run) -> Result<Kept<Run>, ModuleError> {
        self.runs().create(run, &run_text(run)).await.map(kept)
    }

    async fn change(&self, id: &str, version: u64, run: &Run) -> Result<Kept<Run>, ModuleError> {
        self.runs()
            .change(id, version, run, &run_text(run))
            .await
            .map(kept)
    }

    async fn get(&self, id: &str) -> Result<Option<Kept<Run>>, ModuleError> {
        Ok(self.runs().find(id).await?.map(kept))
    }

    async fn of(&self, workflow: &str, limit: usize) -> Result<Vec<Kept<Run>>, ModuleError> {
        self.of_workflow(workflow, limit, |_| true).await
    }

    async fn ended(&self, workflow: &str, count: usize) -> Result<Vec<Kept<Run>>, ModuleError> {
        self.of_workflow(workflow, count, |run| !open(run)).await
    }

    async fn open(&self) -> Result<Vec<Kept<Run>>, ModuleError> {
        let mut open = Vec::new();
        for status in OPEN {
            let example = Run {
                status: status.into(),
                ..Default::default()
            };
            open.extend(
                self.runs()
                    .all(Some((&example, &["status"][..])))
                    .await?
                    .into_iter()
                    .map(kept),
            );
        }
        Ok(open)
    }

    async fn steps(&self, run: &str) -> Result<Vec<Kept<Step>>, ModuleError> {
        let example = Step {
            run: run.to_string(),
            ..Default::default()
        };
        Ok(self
            .steps_of()
            .all(Some((&example, &["run"][..])))
            .await?
            .into_iter()
            .map(kept)
            .collect())
    }

    async fn put_step(&self, step: &Step) -> Result<Kept<Step>, ModuleError> {
        // One document per attempt: the driver, the only writer of its run,
        // has read the journal, so an attempt written before a crash comes
        // to it as an existing step and is changed, never put. Reading here
        // would read every step of every run, for each step.
        let text = format!("{} {}", step.block, step.attempt);
        self.steps_of().create(step, &text).await.map(kept)
    }

    async fn change_step(
        &self,
        id: &str,
        version: u64,
        step: &Step,
    ) -> Result<Kept<Step>, ModuleError> {
        let text = format!("{} {}", step.block, step.attempt);
        self.steps_of()
            .change(id, version, step, &text)
            .await
            .map(kept)
    }
}

#[async_trait]
impl State for DocumentRuns {
    async fn get(
        &self,
        workflow: &str,
        key: &str,
    ) -> Result<Option<Kept<StateEntry>>, ModuleError> {
        let example = StateEntry {
            workflow: workflow.to_string(),
            key: key.to_string(),
            ..Default::default()
        };
        Ok(self
            .entries()
            .all(Some((&example, &["workflow", "key"][..])))
            .await?
            .into_iter()
            .next()
            .map(kept))
    }

    async fn put(&self, entry: &StateEntry, read: Option<(&str, u64)>) -> Result<(), ModuleError> {
        match read {
            None => {
                if State::get(self, &entry.workflow, &entry.key)
                    .await?
                    .is_some()
                {
                    return Err(ModuleError::new(
                        ModuleErrorKind::Conflict,
                        format!("{} is in the store already", entry.key),
                    ));
                }
                self.entries().create(entry, &entry.key).await.map(|_| ())
            }
            Some((id, version)) => self
                .entries()
                .change(id, version, entry, &entry.key)
                .await
                .map(|_| ()),
        }
    }

    async fn delete(&self, id: &str) -> Result<(), ModuleError> {
        self.entries().delete(id).await
    }
}
