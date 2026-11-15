// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! CPU-heavy model work off the async runtime: on the blocking pool, at
//! most as many jobs at once as a semaphore has permits. A caller that
//! stops waiting (its deadline dropped the call, or a timeout passed)
//! leaves the queue at once, and its running job sees the abandon flag
//! and stops at its next step.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::Semaphore;

use crate::error::{Error, Result};

/// Sets the flag when the call that owns it is dropped.
struct Abandon(Arc<AtomicBool>);

impl Drop for Abandon {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

/// Whether the caller of a job has stopped waiting for it.
pub struct Abandoned(Arc<AtomicBool>);

impl Abandoned {
    /// `Err(Abandoned)` once the caller stopped waiting: a job checks it
    /// before each step.
    pub fn check(&self) -> Result<()> {
        if self.0.load(Ordering::Acquire) {
            return Err(Error::Abandoned);
        }
        Ok(())
    }
}

/// Run `job` on the blocking pool once `slots` has a permit, holding the
/// permit until the job returns (also when the caller is gone).
///
/// # Errors
/// `Inactive` when `slots` was closed; `Join` when the job panicked; the
/// job's own error.
pub async fn run<T, F>(slots: &Arc<Semaphore>, job: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce(&Abandoned) -> Result<T> + Send + 'static,
{
    let permit = Arc::clone(slots)
        .acquire_owned()
        .await
        .map_err(|_| Error::Inactive)?;
    let abandon = Abandon(Arc::new(AtomicBool::new(false)));
    let abandoned = Abandoned(Arc::clone(&abandon.0));
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        job(&abandoned)
    })
    .await?
}

#[cfg(test)]
mod tests;
