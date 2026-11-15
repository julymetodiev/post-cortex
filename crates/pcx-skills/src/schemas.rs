// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What the manifest declares: the skill's document kind (its message of
//! `pcx.skills.v1` and its title field, W23) and the module's
//! configuration (the library's workspace).

use pcx_module_api::DocumentKind;
use prost::Name;

use crate::domain::{KIND, Skill};

/// The skill's kind.
pub fn document_kinds() -> Vec<DocumentKind> {
    vec![DocumentKind {
        name: KIND.to_string(),
        title_field: "name".to_string(),
        searchable: None,
        r#type: Skill::full_name(),
    }]
}
