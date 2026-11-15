// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Embedding a document's latest version: one entry (chunked) under its
//! workspace, replacing the version before it.

use crate::content_vectorizer::ContentVectorizer;
use crate::content_vectorizer::ingestion::entries;
use crate::content_vectorizer::vector_text::document_vectors;
use post_cortex_core::core::document::{self, Document};
use post_cortex_ports::error::Result;

impl ContentVectorizer {
    /// Embed `document` (its latest version) and replace the entries its
    /// earlier version had. Returns how many vectors were stored.
    ///
    /// # Errors
    /// The text could not be embedded or the old entries not deleted.
    pub async fn vectorize_document(&self, document: &Document) -> Result<usize> {
        let vectors = document_vectors(document, self.config().max_text_length);
        let texts: Vec<String> = vectors.iter().map(|(text, _)| text.clone()).collect();
        let embeddings = self.embedding_engine().embed(texts).await?;
        if embeddings.len() != vectors.len() {
            return Err(post_cortex_ports::PortError::unavailable(format!(
                "the embedding engine returned {} vectors for {} document texts",
                embeddings.len(),
                vectors.len()
            )));
        }
        // Embedded first, so a failed embedding leaves the old entries.
        self.remove_contents(document.workspace_id, &[document::content_id(document.id)])
            .await?;
        let metadata = vectors.into_iter().map(|(_, meta)| meta).collect();
        let added = self.store_vectors(entries(embeddings, metadata)).await;
        self.invalidate_cached(document.workspace_id);
        Ok(added.len())
    }
}
