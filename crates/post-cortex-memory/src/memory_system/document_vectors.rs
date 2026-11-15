// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Embedding the documents of a workspace: the derived work of a document
//! version (ARCHITECTURE section 7, modules together).

use post_cortex_core::core::document;
use uuid::Uuid;

use crate::memory_system::RuntimeError;
use crate::memory_system::system::MemoryRuntime;

impl MemoryRuntime {
    /// Embed the latest version of each of `ids` of `workspace_id`; a
    /// document deleted meanwhile loses its entry instead. Returns how many
    /// vectors were stored.
    ///
    /// # Errors
    /// A document could not be read, the vectorizer not started, or a
    /// version not embedded.
    pub(crate) async fn vectorize_documents_now(
        &self,
        workspace_id: Uuid,
        ids: &[Uuid],
    ) -> Result<usize, RuntimeError> {
        if !self.config.enable_embeddings || ids.is_empty() {
            return Ok(0);
        }
        let vectorizer = self.ensure_vectorizer_initialized().await?;
        let mut added = 0;
        for &id in ids {
            match self
                .stores
                .documents
                .get_document(workspace_id, id, None)
                .await?
            {
                Some(latest) => added += vectorizer.vectorize_document(&latest).await?,
                None => {
                    self.drop_content_vectors(workspace_id, &[document::content_id(id)])
                        .await;
                }
            }
        }
        Ok(added)
    }
}
