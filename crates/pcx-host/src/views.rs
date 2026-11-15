// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The running modules' views (ARCHITECTURE section 7, Module UI): listed,
//! and one read with its HTML when it is a resource, the module's RPCs
//! its HTML may call as tools and the descriptors its tree's requests are built
//! from.

use pcx_module_api::view::{HTML_MIME, View};
use pcx_module_api::{ModuleError, ModuleErrorKind, Resource, Rpc, Scope};

/// One view of a running module, as [`Host::view`] reads it.
#[derive(Debug, Clone, PartialEq)]
pub struct ModuleView {
    /// The view.
    pub view: View,
    /// Its HTML, when it is a resource.
    pub resource: Option<Resource>,
    /// The module's RPCs its views may call as tools (visibility `app`).
    pub rpcs: Vec<Rpc>,
    /// The module's descriptors, of the RPCs its tree views call.
    pub descriptors: Vec<u8>,
}

impl crate::Host {
    /// Every running module's views, with its id, in load order.
    #[must_use]
    pub fn views(&self) -> Vec<(String, View)> {
        self.loaded
            .load()
            .modules
            .iter()
            .flat_map(|g| {
                let id = g.id().to_string();
                g.module()
                    .manifest()
                    .views
                    .iter()
                    .map(move |view| (id.clone(), view.clone()))
            })
            .collect()
    }

    /// The view `view` of module `module`, with its HTML when it is a
    /// resource (asked of the module within `scope`'s deadline), and the
    /// module's RPCs its views may call as tools.
    ///
    /// # Errors
    /// `NotFound` for an unknown module or view; the module's error;
    /// `Internal` when it answers another resource or another MIME type.
    pub async fn view(
        &self,
        scope: &Scope,
        module: &str,
        view: &str,
    ) -> Result<ModuleView, ModuleError> {
        let loaded = self.loaded.load_full();
        let (index, guarded) = loaded
            .modules
            .iter()
            .enumerate()
            .find(|(_, g)| g.id() == module)
            .ok_or_else(|| {
                ModuleError::new(ModuleErrorKind::NotFound, format!("no module {module}"))
            })?;
        let found = guarded
            .module()
            .manifest()
            .views
            .iter()
            .find(|v| v.id == view)
            .cloned()
            .ok_or_else(|| {
                ModuleError::new(
                    ModuleErrorKind::NotFound,
                    format!("module {module} has no view {view}"),
                )
            })?;
        let rpcs = loaded
            .rpcs
            .iter()
            .filter(|rpc| {
                rpc.module == index && rpc.tool.spec.visibility != pcx_module_api::Visibility::Model
            })
            .filter_map(|rpc| {
                let path = rpc.tool.path();
                guarded
                    .module()
                    .manifest()
                    .rpcs
                    .iter()
                    .find(|r| r.method == path)
                    .cloned()
            })
            .collect();
        let descriptors = guarded.module().manifest().descriptors.clone();
        let Some(pcx_module_api::view::Body::ResourceUri(uri)) = found.body.clone() else {
            return Ok(ModuleView {
                view: found,
                resource: None,
                rpcs,
                descriptors,
            });
        };
        let asked = uri.clone();
        let resource = guarded
            .run(&scope.cancel, scope.deadline, "resource", |m| {
                Box::pin(async move { m.resource(&asked).await })
            })
            .await?;
        if resource.uri != uri || resource.mime_type != HTML_MIME {
            return Err(ModuleError::new(
                ModuleErrorKind::Internal,
                format!(
                    "module {module} answered {} ({}) for {uri}",
                    resource.uri, resource.mime_type
                ),
            ));
        }
        Ok(ModuleView {
            view: found,
            resource: Some(resource),
            rpcs,
            descriptors,
        })
    }
}
