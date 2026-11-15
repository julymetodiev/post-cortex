// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A dry run (docs/modules/workflow.md, triggers and approval): an engine
//! of its own over stores held in memory, its workflow over the kept ones,
//! driven to its end within 5 minutes; nothing is stored, and its run with
//! its journal is the answer.

use std::sync::Arc;
use std::time::Duration;

use pcx_module_api::{ModuleError, Scope};
use post_cortex_proto::workflow::{Fired, Map, RunStatus, Step, Workflow};

use crate::domain::{failure, kinds};
use crate::ports::Kept;
use crate::service::engine::{DryRun, Engine, Input, Ports, Settings, Start};

/// The longest a dry run runs.
const WALL: Duration = Duration::from_secs(300);

/// The id a dry run's workflow has when it is not kept.
const UNKEPT: &str = "dry-run";

impl Engine {
    /// `workflow`, kept or not, run dry by `scope`'s author with `input`,
    /// its `model` and `agent` blocks real when `models`: the run and its
    /// steps.
    ///
    /// # Errors
    /// As [`Engine::start`]'s (but for the approval of its grants); the
    /// stores could not be made.
    pub async fn dry_run(
        self: &Arc<Self>,
        scope: &Scope,
        workflow: &Workflow,
        input: &Map,
        models: bool,
    ) -> Result<(Kept<post_cortex_proto::workflow::Run>, Vec<Step>), ModuleError> {
        let this = match self.ports.workflows.named(&workflow.name).await? {
            Some(kept) => Kept {
                id: kept.id,
                version: kept.version + 1,
                data: workflow.clone(),
            },
            None => Kept {
                id: UNKEPT.to_string(),
                version: 1,
                data: workflow.clone(),
            },
        };
        let stores = self.ports.dry.ports(this)?;
        let dry = Engine::new(
            Ports {
                workflows: stores.workflows,
                runs: stores.runs,
                state: stores.state,
                tools: Arc::clone(&self.ports.tools),
                models: Arc::clone(&self.ports.models),
                turns: Arc::clone(&self.ports.turns),
                events: Arc::clone(&self.ports.events),
                blobs: stores.blobs,
                clock: Arc::clone(&self.ports.clock),
                dry: Arc::clone(&self.ports.dry),
            },
            Settings {
                dry: Some(DryRun { models }),
                ..self.settings.clone()
            },
        )?;
        let fired = Fired {
            author: scope.author.clone().unwrap_or_default(),
            ..Fired::default()
        };
        let run = dry
            .open_run(
                &workflow.name,
                &Start::Dry,
                fired,
                &Input::Map(input.clone()),
                None,
            )
            .await?;
        if let Some(live) = dry.claim(&run.id) {
            let driven = tokio::time::timeout(WALL, dry.driven(&run.id, &live)).await;
            dry.live.lock().remove(&dry.live_key(&run.id));
            match driven {
                Ok(done) => done?,
                Err(_) => {
                    let latest = dry.latest(&run.id).await?;
                    dry.end(
                        &latest,
                        RunStatus::Failed,
                        None,
                        Some(failure(
                            kinds::TIMEOUT,
                            &format!("a dry run ends after {} s", WALL.as_secs()),
                        )),
                    )
                    .await?;
                }
            }
        }
        dry.stop();
        dry.detail(&run.id).await
    }
}
