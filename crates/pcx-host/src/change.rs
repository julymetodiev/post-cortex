// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The running set changed while the host runs, in place, as the composition
//! says (a dynamic module approved or removed, `modules/dynamic.md`).

use std::collections::HashMap;
use std::sync::Arc;

use pcx_module_api::Module;

use crate::{Host, HostError};

impl Host {
    /// The running set with `added` and without the modules of `removed`, in
    /// place: what runs goes on, and no call waits (a dynamic module approved
    /// or removed, `modules/dynamic.md`). They take no configuration.
    ///
    /// # Errors
    /// As [`Host::reload`]; the running set stays.
    pub async fn change(
        &self,
        added: Vec<Arc<dyn Module>>,
        removed: &[String],
    ) -> Result<(), HostError> {
        let before = self.changed.lock().clone();
        let running = self.loaded.load_full();
        let mut base = Vec::with_capacity(running.modules.len());
        let mut policies = HashMap::with_capacity(running.modules.len());
        for (guarded, configured) in running.modules.iter().zip(&running.configured) {
            let module = guarded.module();
            policies.insert(guarded.id().to_string(), configured.policy.clone());
            if !before.iter().any(|m| Arc::ptr_eq(m, module)) {
                base.push(Arc::clone(module));
            }
        }
        {
            let mut changed = self.changed.lock();
            changed.retain(|m| !removed.iter().any(|id| *id == m.manifest().id));
            changed.extend(added);
        }
        let reloaded = self.reload(base, &HashMap::new(), &policies).await;
        if reloaded.is_err() {
            *self.changed.lock() = before;
        }
        reloaded
    }

    /// The names of the events published from here on, for the composition;
    /// none on a host that keeps no events.
    #[must_use]
    pub fn published(&self) -> Option<tokio::sync::broadcast::Receiver<String>> {
        self.bus.published()
    }
}
