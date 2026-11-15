// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A run followed as it goes (docs/modules/workflow.md, the editor): the
//! engine announces each run and step it writes inside the module's
//! process, and a watcher gets the run with its steps, then each change,
//! until the run ends.

use std::sync::Arc;

use pcx_module_api::ModuleError;
use post_cortex_proto::workflow::{Run, Step};
use tokio::sync::{broadcast, mpsc};

use crate::domain::open;
use crate::ports::Kept;
use crate::service::engine::Engine;

/// Changes the engine holds for watchers that are behind; one further
/// behind is sent its run again.
pub const BACKLOG: usize = 256;

/// Changes a watcher is sent before it reads them.
const QUEUE: usize = 64;

/// What the engine wrote of a run.
#[derive(Debug, Clone)]
pub enum Written {
    /// The run.
    Run(Box<Kept<Run>>),
    /// One of its steps.
    Step(Box<Step>),
}

/// What a watched run answers.
#[derive(Debug, Clone)]
pub enum Watched {
    /// The run with its steps, in the order they started.
    Whole(Box<Kept<Run>>, Vec<Step>),
    /// The run, changed.
    Run(Box<Kept<Run>>),
    /// One of its steps, written.
    Step(Box<Step>),
}

/// A watched run's changes; the channel closes after the run ends.
pub type Watch = mpsc::Receiver<Result<Watched, ModuleError>>;

impl Engine {
    /// `written` told to the runs' watchers.
    pub fn wrote(&self, written: Written) {
        if self.watched.receiver_count() == 0 {
            return;
        }
        if self.watched.send(written).is_err() {
            tracing::trace!("the watchers left before a write reached them");
        }
    }

    /// Run `id` with its steps, in the order they started.
    ///
    /// # Errors
    /// `NotFound`; memory failed.
    pub async fn detail(&self, id: &str) -> Result<(Kept<Run>, Vec<Step>), ModuleError> {
        let run = self.latest(id).await?;
        let mut steps: Vec<Step> = self
            .steps(id)
            .await?
            .into_values()
            .map(|k| k.data)
            .collect();
        steps.sort_by_key(|s| (s.started_at.map(|t| (t.seconds, t.nanos)), s.attempt));
        Ok((run, steps))
    }

    /// Run `id` followed: the run with its steps, then each change of it
    /// or a step as it is written, sent again whole when the watcher fell
    /// behind; the channel closes after the run ends or when the watcher
    /// leaves.
    ///
    /// # Errors
    /// `NotFound`; memory failed.
    pub async fn watch(self: &Arc<Self>, id: &str) -> Result<Watch, ModuleError> {
        // Listening before reading: nothing written between them is lost.
        let mut heard = self.watched.subscribe();
        let (run, steps) = self.detail(id).await?;
        let (send, watch) = mpsc::channel(QUEUE);
        let ended = !open(&run.data);
        if send
            .send(Ok(Watched::Whole(Box::new(run), steps)))
            .await
            .is_err()
            || ended
        {
            return Ok(watch);
        }
        let engine = Arc::clone(self);
        let id = id.to_string();
        tokio::spawn(async move {
            loop {
                let change = tokio::select! {
                    () = send.closed() => return,
                    change = heard.recv() => change,
                };
                let (watched, ended) = match change {
                    Ok(Written::Run(run)) if run.id == id => {
                        let ended = !open(&run.data);
                        (Ok(Watched::Run(run)), ended)
                    }
                    Ok(Written::Step(step)) if step.run == id => (Ok(Watched::Step(step)), false),
                    Ok(_) => continue,
                    Err(broadcast::error::RecvError::Lagged(_)) => match engine.detail(&id).await {
                        Ok((run, steps)) => {
                            let ended = !open(&run.data);
                            (Ok(Watched::Whole(Box::new(run), steps)), ended)
                        }
                        Err(e) => (Err(e), true),
                    },
                    Err(broadcast::error::RecvError::Closed) => return,
                };
                if send.send(watched).await.is_err() || ended {
                    return;
                }
            }
        });
        Ok(watch)
    }
}
