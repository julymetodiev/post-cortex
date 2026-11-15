// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! One engine, every workspace (docs/modules/workflow.md, its workspaces):
//! what the engine holds (the runs being driven, the `event` triggers, the
//! watchers, its own messages) is one, and it is at work in a workspace
//! through that workspace's ports and settings, which [`Spaces`] gives.

use std::sync::Arc;

use pcx_module_api::ModuleError;

use crate::service::engine::{Engine, Ports, Settings};

/// The engine's ports and settings in each workspace.
pub trait Spaces: Send + Sync {
    /// What the engine reaches `workspace` through, and its settings there.
    fn of(&self, workspace: &str) -> (Ports, Settings);
}

impl Engine {
    /// The engine of every workspace, at work in `workspace`: its ports and
    /// settings in each are `spaces`'.
    ///
    /// # Errors
    /// `Internal` when the module's descriptors do not read.
    pub fn of_spaces(spaces: Arc<dyn Spaces>, workspace: &str) -> Result<Arc<Self>, ModuleError> {
        let (ports, settings) = spaces.of(workspace);
        Self::built(ports, settings, Some(spaces))
    }

    /// This engine at work in `workspace`: the same runs being driven, the
    /// same triggers and watchers, over that workspace's ports. One that
    /// works in one workspace only is itself.
    #[must_use]
    pub fn at(self: &Arc<Self>, workspace: &str) -> Arc<Self> {
        let Some(spaces) = self
            .spaces
            .as_ref()
            .filter(|_| workspace != self.settings.workspace)
        else {
            return Arc::clone(self);
        };
        let (ports, settings) = spaces.of(workspace);
        Arc::new(Self {
            ports,
            settings,
            spaces: Some(Arc::clone(spaces)),
            own: Arc::clone(&self.own),
            boards: Arc::clone(&self.boards),
            live: Arc::clone(&self.live),
            listeners: Arc::clone(&self.listeners),
            stopping: self.stopping.clone(),
            watched: self.watched.clone(),
        })
    }

    /// The workspace it works in.
    #[must_use]
    pub fn workspace(&self) -> &str {
        &self.settings.workspace
    }

    /// Run `id` of its workspace, as the runs being driven are kept: a
    /// run's id is its document's, unique in its workspace.
    #[must_use]
    pub fn live_key(&self, id: &str) -> (String, String) {
        (self.settings.workspace.clone(), id.to_string())
    }
}
