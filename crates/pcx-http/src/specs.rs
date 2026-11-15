// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The http module's RPCs as tools (docs/modules/http.md, tools), and its
//! log's and presets' document kinds as its manifest declares them.

use pcx_module_api::DocumentKind;
use post_cortex_proto::http::{Call, Preset};
use post_cortex_proto::module::{Effect, Rpc, ToolVisibility};
use prost::Name;

use crate::domain::{CALL_KIND, PRESET_KIND};

/// The module's service.
pub const SERVICE: &str = "pcx.http.v1.Http";

/// Each RPC of the service, its tool name, its effect, and whether only a
/// person calls it (the panel's test button).
const RPCS: [(&str, &str, Effect, bool); 9] = [
    ("Get", "http_get", Effect::Read, false),
    ("Request", "http_request", Effect::External, false),
    ("ListConnections", "http_connections", Effect::Read, false),
    ("TestConnection", "http_test", Effect::Read, true),
    ("ListPresets", "http_presets_list", Effect::Read, false),
    ("GetPreset", "http_presets_get", Effect::Read, false),
    ("SavePreset", "http_presets_save", Effect::Write, false),
    ("DeletePreset", "http_presets_delete", Effect::Write, false),
    ("ListLog", "http_log", Effect::Read, false),
];

/// Every RPC of the module, as its manifest names it.
pub fn rpcs() -> Vec<Rpc> {
    RPCS.iter()
        .map(|(method, tool, effect, app)| Rpc {
            method: format!("/{SERVICE}/{method}"),
            tool: (*tool).to_string(),
            visibility: if *app {
                vec![ToolVisibility::App.into()]
            } else {
                Vec::new()
            },
            effect: (*effect).into(),
        })
        .collect()
}

/// Its kinds: a call titled by its method and host, kept out of search; a
/// preset titled by its name.
pub fn document_kinds() -> Vec<DocumentKind> {
    vec![
        DocumentKind {
            name: CALL_KIND.to_string(),
            title_field: "title".to_string(),
            searchable: Some(false),
            r#type: Call::full_name(),
        },
        DocumentKind {
            name: PRESET_KIND.to_string(),
            title_field: "name".to_string(),
            searchable: None,
            r#type: Preset::full_name(),
        },
    ]
}
