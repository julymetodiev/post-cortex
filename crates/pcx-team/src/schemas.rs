// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The team's document kinds as its manifest declares them: each kind's
//! message of `pcx.team.v1` and its title field (W23).

use pcx_module_api::DocumentKind;
use prost::Name;

use crate::autonomy::Autonomy;
use crate::domain::{Agent, Kind, Team, Work};

/// The message of `kind`'s data and its title field.
fn message(kind: Kind) -> (String, &'static str) {
    match kind {
        Kind::Team => (Team::full_name(), "name"),
        Kind::Agent => (Agent::full_name(), "name"),
        Kind::Autonomy => (Autonomy::full_name(), "team"),
        Kind::Work => (Work::full_name(), "agent"),
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
