// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What the manifest declares: the chief's document kinds (each one's
//! message of `pcx.chief.v1` and its title field, W23).

use pcx_module_api::DocumentKind;
use prost::Name;

use crate::domain::{Action, Day, Kind, Talk};

/// The message of `kind`'s data and its title field.
fn message(kind: Kind) -> (String, &'static str) {
    match kind {
        Kind::Talk => (Talk::full_name(), "asked_by"),
        Kind::Action => (Action::full_name(), "tool"),
        Kind::Day => (Day::full_name(), "date"),
    }
}

/// Every kind, as the manifest declares it.
pub fn document_kinds() -> Vec<DocumentKind> {
    Kind::ALL
        .iter()
        .map(|kind| {
            let (r#type, title_field) = message(*kind);
            DocumentKind {
                name: kind.name().to_string(),
                title_field: title_field.to_string(),
                searchable: None,
                r#type,
            }
        })
        .collect()
}
