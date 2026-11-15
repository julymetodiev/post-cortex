// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The descriptors the tools of several modules are built from: one pool
//! of all of them, so a message of one module that carries another's (an
//! event's payload, a `google.protobuf.Any`) maps to JSON (docs/host.md,
//! a tool's answer).

use prost_reflect::DescriptorPool;

use crate::Manifest;

/// Whether every file of `own` is new to `shared` or the same file there.
fn agrees(shared: &DescriptorPool, own: &DescriptorPool) -> bool {
    own.files().all(|file| {
        shared
            .get_file_by_name(file.name())
            .is_none_or(|kept| kept.encode_to_vec() == file.encode_to_vec())
    })
}

/// The pool each of `manifests` builds its tools from, in their order: one
/// pool of the descriptors of all of them, but its own for a module that
/// has a file of a name another has with other content (a pool keeps one
/// file of a name), which no other module's messages then reach.
///
/// # Errors
/// The index of a manifest whose descriptors do not read, and why.
pub fn pools(manifests: &[&Manifest]) -> Result<Vec<DescriptorPool>, (usize, String)> {
    let mut shared = DescriptorPool::new();
    let mut apart = Vec::with_capacity(manifests.len());
    for (index, manifest) in manifests.iter().enumerate() {
        let own = crate::descriptors(&manifest.descriptors).map_err(|e| (index, e))?;
        if agrees(&shared, &own) {
            shared
                .decode_file_descriptor_set(manifest.descriptors.as_slice())
                .map_err(|e| (index, format!("its descriptors do not read: {e}")))?;
            apart.push(None);
        } else {
            tracing::warn!(
                module = %manifest.id,
                "a file of its descriptors differs from a running module's of the same name: its tools read no other module's messages"
            );
            apart.push(Some(own));
        }
    }
    Ok(apart
        .into_iter()
        .map(|own| own.unwrap_or_else(|| shared.clone()))
        .collect())
}

#[cfg(test)]
mod tests;
