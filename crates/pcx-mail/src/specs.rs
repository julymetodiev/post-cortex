// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The mail module's RPCs as tools (docs/modules/mail.md), and its log's
//! document kind as its manifest declares it.

use pcx_module_api::DocumentKind;
use post_cortex_proto::mail::{Sent, Template};
use post_cortex_proto::module::{Effect, Rpc, ToolVisibility};
use prost::Name;

use crate::domain::SENT_KIND;
use crate::domain::template::TEMPLATE_KIND;

/// The module's service.
pub const SERVICE: &str = "pcx.mail.v1.Mail";

/// Each RPC of the service, its tool name, its effect, and whether only a
/// person calls it (the panel's test button).
const RPCS: [(&str, &str, Effect, bool); 7] = [
    ("Send", "mail_send", Effect::External, false),
    ("Test", "mail_test", Effect::External, true),
    ("ListLog", "mail_log", Effect::Read, false),
    ("ListTemplates", "mail_templates_list", Effect::Read, false),
    ("GetTemplate", "mail_templates_get", Effect::Read, false),
    ("SaveTemplate", "mail_templates_save", Effect::Write, false),
    (
        "PreviewTemplate",
        "mail_templates_preview",
        Effect::Read,
        false,
    ),
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

/// Its kinds: a send titled by its subject, kept out of search; a
/// template titled by its name.
pub fn document_kinds() -> Vec<DocumentKind> {
    vec![
        DocumentKind {
            name: SENT_KIND.to_string(),
            title_field: "subject".to_string(),
            searchable: Some(false),
            r#type: Sent::full_name(),
        },
        DocumentKind {
            name: TEMPLATE_KIND.to_string(),
            title_field: "name".to_string(),
            searchable: None,
            r#type: Template::full_name(),
        },
    ]
}
