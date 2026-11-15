// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A provider's listing at start, in the background (docs/modules/agent.md,
//! the models; owner 2026-11-01): a start does not wait on the provider's
//! API. Its models are its table's until the listing is in; a refused key is the listing's failure, which its
//! `Models` answer carries.

use std::future::Future;
use std::sync::Arc;

use parking_lot::{Mutex, RwLock};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::model::{ModelFailure, ModelInfo, Table};
use crate::{ModuleError, ModuleErrorKind};

/// The outcome of a listing: `None` while it runs.
type Outcome = Option<Result<(), ModuleError>>;

/// A provider's models and the listing its last start began.
pub struct Listed {
    models: Arc<RwLock<Vec<ModelInfo>>>,
    settled: RwLock<Option<watch::Receiver<Outcome>>>,
    lister: Mutex<Option<JoinHandle<()>>>,
}

impl Listed {
    /// `models` (a table's), no listing begun.
    #[must_use]
    pub fn new(models: Vec<ModelInfo>) -> Self {
        Self {
            models: Arc::new(RwLock::new(models)),
            settled: RwLock::new(None),
            lister: Mutex::new(None),
        }
    }

    /// Its models as they are now.
    #[must_use]
    pub fn models(&self) -> Vec<ModelInfo> {
        self.models.read().clone()
    }

    /// Begin listing `provider`'s models, stopping a listing begun before:
    /// now its `table`'s, then what `served` gives, read by `table`.
    pub fn begin<S>(&self, provider: &'static str, table: Table, served: S)
    where
        S: Future<Output = Result<Vec<ModelInfo>, ModelFailure>> + Send + 'static,
    {
        if let Some(before) = self.lister.lock().take() {
            before.abort();
        }
        *self.models.write() = table.listing(None);
        let (settle, settled) = watch::channel(None);
        let models = Arc::clone(&self.models);
        let lister = tokio::spawn(async move {
            let outcome = match table.listed(provider, served.await) {
                Ok(listed) => {
                    *models.write() = listed;
                    Ok(())
                }
                Err(refused) => {
                    tracing::warn!(provider, error = %refused.message, "its key was refused");
                    Err(
                        ModuleError::new(ModuleErrorKind::InvalidArgument, refused.message)
                            .with_field("api_key"),
                    )
                }
            };
            settle.send_replace(Some(outcome));
        });
        *self.settled.write() = Some(settled);
        *self.lister.lock() = Some(lister);
    }

    /// Its models once the listing begun is in.
    ///
    /// # Errors
    /// The listing was refused (its key).
    pub async fn listed(&self) -> Result<Vec<ModelInfo>, ModuleError> {
        let settled = self.settled.read().clone();
        if let Some(mut settled) = settled {
            // A listing stopped with its provider leaves the models it had.
            if let Ok(outcome) = settled.wait_for(Option::is_some).await
                && let Some(Err(refused)) = outcome.as_ref()
            {
                return Err(refused.clone());
            }
        }
        Ok(self.models())
    }

    /// Stop the listing begun, if it still runs.
    pub fn stop(&self) {
        if let Some(lister) = self.lister.lock().take() {
            lister.abort();
        }
    }
}
