// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The board's document kinds as its manifest declares them: each kind's
//! message of `pcx.board.v1` and its title field (W23).

use pcx_module_api::DocumentKind;
use prost::Name;

use crate::domain::{Board, Comment, Decision, Kind, Task};

/// The message of `kind`'s data and its title field.
fn message(kind: Kind) -> (String, &'static str) {
    match kind {
        Kind::Board => (Board::full_name(), "name"),
        Kind::Task => (Task::full_name(), "title"),
        Kind::Comment => (Comment::full_name(), "title"),
        Kind::Decision => (Decision::full_name(), "what"),
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
