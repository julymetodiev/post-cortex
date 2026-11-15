// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The embedding model of an archive's vectors (v1.4.1+): recorded at
//! export, compared with the target's at import. Vectors of another model
//! are meaningless to the target's index (another space, often another
//! dimension), so an import refuses them, writing nothing, and a preview
//! warns.

use post_cortex_core::archive::{EmbeddingModelInfo, ExportedEmbedding};

/// What an archive says about its vectors, learnt while it is read.
#[derive(Debug, Clone, Default)]
pub struct ArchivedVectors {
    /// The model its header records (none before v1.4.1).
    pub model: Option<EmbeddingModelInfo>,
    /// How many vectors it holds.
    pub count: usize,
    /// The length of its first vector.
    pub first_dimension: Option<usize>,
}

impl ArchivedVectors {
    /// Count `vector`.
    pub fn add(&mut self, vector: &ExportedEmbedding) {
        self.count += 1;
        self.first_dimension.get_or_insert(vector.vector.len());
    }
}

/// Why the archived vectors do not fit a store that embeds with `target`,
/// or `None` when they do (or there are none, or the target is unknown). An
/// archive that does not record its model (before v1.4.1) is compared by
/// the length of its vectors.
pub fn mismatch(archived: &ArchivedVectors, target: Option<&EmbeddingModelInfo>) -> Option<String> {
    let target = target?;
    let first = archived.first_dimension?;
    match &archived.model {
        Some(archive) if archive != target => Some(format!(
            "the archive's {} vectors were embedded with {} ({} dimensions), this store embeds \
             with {} ({} dimensions)",
            archived.count, archive.model, archive.dimension, target.model, target.dimension
        )),
        Some(_) => None,
        None if first != target.dimension => Some(format!(
            "the archive's {} vectors have {} dimensions, this store's model {} has {}",
            archived.count, first, target.model, target.dimension
        )),
        None => None,
    }
}
