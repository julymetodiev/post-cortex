// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Starting and stopping a load's modules: all of a load's new modules
//! start at once, and stop last loaded first.

use std::sync::Arc;

use pcx_module_api::{HostContext, ModuleError};

use crate::policy::Guarded;

/// Start `modules` at once, each with its context (a host starts in the
/// time of its slowest module): those that started, in their order, and
/// those that did not, each with why. The caller stops the started ones
/// when a failure ends its load.
pub async fn start_each(
    modules: Vec<(Arc<Guarded>, HostContext)>,
) -> (Vec<Arc<Guarded>>, Vec<(Arc<Guarded>, ModuleError)>) {
    let outcomes =
        futures::future::join_all(modules.into_iter().map(|(guarded, ctx)| async move {
            let starting = std::time::Instant::now();
            let outcome = guarded.module().start(ctx).await;
            (guarded, starting.elapsed(), outcome)
        }))
        .await;
    let mut started = Vec::with_capacity(outcomes.len());
    let mut failed = Vec::new();
    for (guarded, took, outcome) in outcomes {
        match outcome {
            Ok(()) => {
                tracing::info!(
                    module = guarded.id(),
                    ms = took.as_millis() as u64,
                    "module started"
                );
                started.push(guarded);
            }
            Err(error) => failed.push((guarded, error)),
        }
    }
    (started, failed)
}

/// Stop `modules`, last first; one that does not stop cleanly is logged.
pub async fn stop_all(modules: &[Arc<Guarded>]) {
    for guarded in modules.iter().rev() {
        if let Err(e) = guarded.module().stop().await {
            tracing::warn!(module = guarded.id(), error = %e, "module did not stop cleanly");
        }
    }
}
