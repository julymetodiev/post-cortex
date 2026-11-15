// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The one load path's checks (T5, T6): API version, manifest (with its
//! configuration message and document kinds), unique ids, one tool on an RPC's path, exclusive slots; once started, its
//! tools and the views over them.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use pcx_module_api::{
    ApiVersion, ConfigType, DeclaredKind, EventKind, Manifest, Module, ModuleKind, ToolSpec,
    compatible, view,
};

use pcx_module_api::bus::check_emits;
use pcx_module_api::model::check_info;
use pcx_module_api::sandbox::Policy;

use crate::HostError;
use crate::policy::Guarded;

/// The exclusive slot of `kind`, if it has one.
fn exclusive_slot(kind: ModuleKind) -> Option<&'static str> {
    match kind {
        ModuleKind::Memory => Some("memory"),
        ModuleKind::ContextEngine => Some("context_engine"),
        ModuleKind::Agent => Some("agent"),
        ModuleKind::Sandbox => Some("sandbox"),
        _ => None,
    }
}

/// Refuse a manifest the host cannot run; its configuration schema, if it
/// declares one.
pub fn check_manifest(
    host: &ApiVersion,
    manifest: &Manifest,
) -> Result<Option<ConfigType>, HostError> {
    let id = manifest.id.as_str();
    let invalid = |reason: String| HostError::InvalidManifest {
        module: id.to_string(),
        reason,
    };
    if id.trim().is_empty() {
        return Err(invalid("no id".to_string()));
    }
    let api = manifest.api.unwrap_or_default();
    if !compatible(host, &api) {
        return Err(HostError::IncompatibleApi {
            module: id.to_string(),
            module_api: api,
            host_api: *host,
        });
    }
    kind_of(manifest).ok_or_else(|| invalid("no module kind".to_string()))?;
    for subscription in &manifest.events {
        EventKind::try_from(subscription.kind)
            .ok()
            .filter(|k| *k != EventKind::Unspecified)
            .ok_or_else(|| invalid("an event subscription without a known kind".to_string()))?;
    }
    DeclaredKind::of(manifest).map_err(|e| invalid(e.to_string()))?;
    check_emits(manifest).map_err(invalid)?;
    ConfigType::of(manifest).map_err(|e| invalid(e.to_string()))
}

/// What `policies` grants module `id`: nothing when it names none.
pub fn granted(policies: &HashMap<String, Policy>, id: &str) -> Policy {
    policies.get(id).cloned().unwrap_or_default()
}

/// The kind a manifest declares; none when it declares no known one.
pub fn kind_of(manifest: &Manifest) -> Option<ModuleKind> {
    ModuleKind::try_from(manifest.kind)
        .ok()
        .filter(|k| *k != ModuleKind::Unspecified)
}

/// Refuse a set of modules the host cannot run together; each one's
/// configuration schema, in order.
pub fn validate(
    host: &ApiVersion,
    modules: &[Arc<dyn Module>],
    policies: &HashMap<String, Policy>,
) -> Result<Vec<Option<ConfigType>>, HostError> {
    let mut ids: HashSet<&str> = HashSet::new();
    let mut slots: HashMap<&'static str, &str> = HashMap::new();
    let mut models: HashMap<String, &str> = HashMap::new();
    let mut tool_paths: HashMap<&str, &str> = HashMap::new();
    let mut schemas = Vec::with_capacity(modules.len());
    for module in modules {
        let manifest = module.manifest();
        schemas.push(check_manifest(host, manifest)?);
        let id = manifest.id.as_str();
        if !ids.insert(id) {
            return Err(HostError::DuplicateId(id.to_string()));
        }
        check_grants(module.as_ref(), &granted(policies, id))?;
        for rpc in manifest.rpcs.iter().filter(|rpc| !rpc.tool.is_empty()) {
            if let Some(first) = tool_paths.insert(rpc.method.as_str(), id) {
                return Err(HostError::DuplicateToolPath {
                    path: rpc.method.clone(),
                    first: first.to_string(),
                    module: id.to_string(),
                });
            }
        }
        for model in module.model_provider().iter().flat_map(|p| p.models()) {
            check_info(&model).map_err(|source| HostError::InvalidModel {
                module: id.to_string(),
                source,
            })?;
            if let Some(first) = models.insert(model.id.clone(), id) {
                return Err(HostError::DuplicateModel {
                    model: model.id,
                    first: first.to_string(),
                    module: id.to_string(),
                });
            }
        }
        if let Some(slot) = kind_of(manifest).and_then(exclusive_slot)
            && let Some(existing) = slots.insert(slot, id)
        {
            return Err(HostError::SlotTaken {
                slot,
                existing: existing.to_string(),
                module: id.to_string(),
            });
        }
    }
    check_follows(modules)?;
    Ok(schemas)
}

/// Refuse a module that follows an event (or the events of a module) no
/// module it uses emits.
fn check_follows(modules: &[Arc<dyn Module>]) -> Result<(), HostError> {
    let running: Vec<&Manifest> = modules.iter().map(|m| m.manifest()).collect();
    for manifest in &running {
        pcx_module_api::bus::followed(manifest, &running).map_err(|reason| {
            HostError::InvalidManifest {
                module: manifest.id.clone(),
                reason,
            }
        })?;
    }
    Ok(())
}

/// Refuse a module the host cannot give what it asks: a sandbox that
/// provides no sandboxes, a model provider that serves no models, or a filesystem path outside the roots its
/// configuration grants (or a write the root does not allow).
fn check_grants(module: &dyn Module, policy: &Policy) -> Result<(), HostError> {
    let manifest = module.manifest();
    let invalid = |reason: String| HostError::InvalidManifest {
        module: manifest.id.clone(),
        reason,
    };
    if kind_of(manifest) == Some(ModuleKind::Sandbox) && module.sandbox_provider().is_none() {
        return Err(invalid(
            "a sandbox module provides no sandboxes (a sandbox runs in the host process)"
                .to_string(),
        ));
    }
    if kind_of(manifest) == Some(ModuleKind::ModelProvider) && module.model_provider().is_none() {
        return Err(invalid("a model provider serves no models".to_string()));
    }
    let agent = kind_of(manifest) == Some(ModuleKind::Agent);
    if agent && manifest.turns {
        return Err(invalid(
            "the agent declares no turns of its own: its slot's are its turns".to_string(),
        ));
    }
    if !agent && manifest.turns != module.turns().is_some() {
        return Err(invalid(if manifest.turns {
            "it declares turns but runs none".to_string()
        } else {
            "it runs turns it does not declare (Manifest.turns)".to_string()
        }));
    }
    for asked in &manifest.filesystem {
        let path = std::path::Path::new(&asked.path);
        let lexical = path.is_absolute()
            && !path
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir));
        let granted = lexical
            && policy
                .roots
                .iter()
                .any(|root| path.starts_with(&root.path) && (root.write || !asked.write));
        if !granted {
            return Err(invalid(format!(
                "it asks for {} {}, outside the roots its configuration grants",
                if asked.write { "writing" } else { "reading" },
                asked.path
            )));
        }
    }
    Ok(())
}

/// Every started module's tools MCP clients see, with the index of its
/// module: its MCP tools when it has them (memory's), else the RPCs it
/// names as tools (`rpcs`, W23); refused when two modules offer a tool of
/// one name (an RPC of a module with MCP tools counts too), or a module's
/// views do not fit its RPCs.
pub fn tools(
    modules: &[Arc<Guarded>],
    rpcs: &[crate::rpc_tools::HostRpc],
) -> Result<Vec<(usize, ToolSpec)>, HostError> {
    let mut offered: HashMap<String, &str> = HashMap::new();
    let mut tools = Vec::new();
    for (index, guarded) in modules.iter().enumerate() {
        view::check(guarded.module().manifest()).map_err(|error| HostError::InvalidView {
            module: guarded.id().to_string(),
            error,
        })?;
        let mcp = guarded.module().mcp_tools();
        let own: Vec<ToolSpec> = rpcs
            .iter()
            .filter(|rpc| rpc.module == index)
            .map(|rpc| rpc.tool.spec.clone())
            .collect();
        for tool in mcp.iter().chain(&own) {
            if let Some(first) = offered.insert(tool.name.clone(), guarded.id()) {
                return Err(HostError::DuplicateTool {
                    tool: tool.name.clone(),
                    first: first.to_string(),
                    module: guarded.id().to_string(),
                });
            }
        }
        let listed = if mcp.is_empty() { own } else { mcp };
        tools.extend(listed.into_iter().map(|tool| (index, tool)));
    }
    Ok(tools)
}
