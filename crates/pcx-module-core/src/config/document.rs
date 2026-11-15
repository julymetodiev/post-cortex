// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The kinds of documents a module keeps in memory (ARCHITECTURE section 7,
//! modules together): the one check of a manifest's `document_kinds`, so
//! every host and the cloud refuse the same kinds. A kind's data is a
//! message of the module's descriptors (W23).

use std::collections::HashSet;
use std::fmt;

use prost_reflect::{Cardinality, DescriptorPool, DynamicMessage, Kind};

use crate::Manifest;

/// A document kind a manifest declares, checked.
#[derive(Debug, Clone, PartialEq)]
pub struct DeclaredKind {
    /// The module that declares it.
    pub module: String,
    /// Its name in the module.
    pub name: String,
    /// The full name of the message of its data.
    pub type_name: String,
    /// The string field of that message that is its title.
    pub title_field: String,
}

/// A document kind its module cannot declare; the module is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KindError {
    /// The kind's name (empty when it has none).
    pub kind: String,
    /// What is wrong.
    pub reason: String,
}

impl fmt::Display for KindError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.kind.is_empty() {
            write!(f, "a document kind {}", self.reason)
        } else {
            write!(f, "document kind {} {}", self.kind, self.reason)
        }
    }
}

impl std::error::Error for KindError {}

/// The messages of a module's descriptors (`Manifest.descriptors`).
///
/// # Errors
/// They are not an encoded `FileDescriptorSet` whose files link. Read as
/// they are, the options keep their extensions (a setting's `setting`).
pub fn descriptors(encoded: &[u8]) -> Result<DescriptorPool, String> {
    DescriptorPool::decode(encoded).map_err(|e| format!("its descriptors do not read: {e}"))
}

/// Whether document data `message` has each of `fields` (top-level, by
/// name) equal to `example`'s: the one meaning of a document filter, for
/// memory and every double of it.
#[must_use]
pub fn data_matches(message: &DynamicMessage, example: &DynamicMessage, fields: &[String]) -> bool {
    fields.iter().all(|field| {
        match (
            message.get_field_by_name(field),
            example.get_field_by_name(field),
        ) {
            (Some(have), Some(want)) => have == want,
            _ => false,
        }
    })
}

impl DeclaredKind {
    /// The document kinds of `manifest`: each name of lowercase letters,
    /// digits, `_` and `-`, unique in the module; each type a message of
    /// the manifest's descriptors; each title field a singular string field
    /// of it.
    ///
    /// # Errors
    /// The first kind at fault and why.
    pub fn of(manifest: &Manifest) -> Result<Vec<Self>, KindError> {
        if manifest.document_kinds.is_empty() {
            return Ok(Vec::new());
        }
        let pool = descriptors(&manifest.descriptors).map_err(|reason| KindError {
            kind: String::new(),
            reason: format!("cannot be read: {reason}"),
        })?;
        let mut names = HashSet::new();
        let mut kinds = Vec::with_capacity(manifest.document_kinds.len());
        for declared in &manifest.document_kinds {
            let name = declared.name.as_str();
            let refused = |reason: String| KindError {
                kind: name.to_string(),
                reason,
            };
            let well_formed = !name.is_empty()
                && name.bytes().all(|b| {
                    b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-'
                });
            if !well_formed {
                return Err(refused(
                    "needs a name of lowercase letters, digits, _ and -".to_string(),
                ));
            }
            if !names.insert(name) {
                return Err(refused("is declared twice".to_string()));
            }
            let message = pool.get_message_by_name(&declared.r#type).ok_or_else(|| {
                refused(format!(
                    "names the type {:?}, not a message of the module's descriptors",
                    declared.r#type
                ))
            })?;
            let title = message
                .get_field_by_name(&declared.title_field)
                .filter(|f| f.kind() == Kind::String && f.cardinality() != Cardinality::Repeated);
            if title.is_none() {
                return Err(refused(format!(
                    "names the title field {:?}, not a singular string field of {}",
                    declared.title_field, declared.r#type
                )));
            }
            kinds.push(Self {
                module: manifest.id.clone(),
                name: name.to_string(),
                type_name: declared.r#type.clone(),
                title_field: declared.title_field.clone(),
            });
        }
        Ok(kinds)
    }
}

#[cfg(test)]
mod tests;
