// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The `dynamic` module's RPCs as tools, its document kind, its events.

use pcx_module_api::DocumentKind;
use post_cortex_proto::dynamic::{DynamicModule, VersionApproved, VersionRemoved};
use post_cortex_proto::module::{Effect, EmittedEvent, Rpc, ToolVisibility};
use prost::Name;

use crate::domain::{KIND_NAME, MODULE_ID};

/// The module's service.
pub const SERVICE: &str = "pcx.dynamic.v1.Dynamic";

/// The event of a version approved.
pub const VERSION_APPROVED: &str = "dynamic.version_approved";
/// The event of a module removed.
pub const VERSION_REMOVED: &str = "dynamic.version_removed";

/// Each RPC, its tool (none: no agent calls it), whether only its view calls
/// that tool, and its effect. An agent reads with `Get` and writes with
/// `Install`.
const RPCS: [(&str, &str, bool, Effect); 7] = [
    ("Install", "dynamic_install", false, Effect::External),
    ("List", "dynamic_list", false, Effect::Read),
    ("Get", "dynamic_get", false, Effect::Read),
    ("Edit", "dynamic_edit", true, Effect::External),
    ("Files", "dynamic_files", true, Effect::Read),
    ("Remove", "dynamic_remove", false, Effect::Write),
    ("ListApproved", "", false, Effect::Read),
];

/// An RPC's path.
#[must_use]
pub fn path(method: &str) -> String {
    format!("/{SERVICE}/{method}")
}

/// Every RPC of the module, as its manifest names it.
pub fn rpcs() -> Vec<Rpc> {
    RPCS.iter()
        .map(|(method, tool, view_only, effect)| Rpc {
            method: path(method),
            tool: (*tool).to_string(),
            visibility: if *view_only {
                vec![ToolVisibility::App.into()]
            } else {
                Vec::new()
            },
            effect: (*effect).into(),
        })
        .collect()
}

/// Its kind: a module titled by its id, found by search.
pub fn document_kinds() -> Vec<DocumentKind> {
    vec![DocumentKind {
        name: KIND_NAME.to_string(),
        title_field: "id".to_string(),
        searchable: None,
        r#type: DynamicModule::full_name(),
    }]
}

/// The events it emits.
pub fn emits() -> Vec<EmittedEvent> {
    vec![
        EmittedEvent {
            name: VERSION_APPROVED.to_string(),
            r#type: VersionApproved::full_name(),
            description: format!("A {MODULE_ID} module's version was approved."),
        },
        EmittedEvent {
            name: VERSION_REMOVED.to_string(),
            r#type: VersionRemoved::full_name(),
            description: format!("A {MODULE_ID} module was removed."),
        },
    ]
}
