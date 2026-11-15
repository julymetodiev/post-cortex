// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Attaching the rerankers the ports carry to the vectorizer.

use crate::memory_system::MemoryRuntime;
use std::sync::Arc;
use tracing::warn;

impl MemoryRuntime {
    /// Set up the rerankers the ports carry: the local one serves `local`
    /// mode, the accurate one (e.g. Voyage with the local one as fallback)
    /// `accurate` mode. A local reranker that fails to load (its first
    /// score loads it) is logged and left out, and searches in its mode
    /// fall back to a cheaper one rather than fail.
    pub async fn attach_reranker(
        &self,
        vectorizer: &mut crate::content_vectorizer::ContentVectorizer,
    ) {
        let (local, accurate) = (self.reranker(), self.accurate_reranker());
        if let Some(local) = local {
            match local.score("", &[]).await {
                Ok(_) => {
                    tracing::debug!("Reranking search results locally");
                    vectorizer.set_reranker(Arc::clone(local));
                }
                Err(e) => warn!("Failed to load the reranker: {:#}", e),
            }
        }
        if let Some(accurate) = accurate {
            vectorizer.set_accurate_reranker(Arc::clone(accurate));
        }
        if let Some(mode) = &self.embedding_config_holder.search_mode {
            match mode.parse::<crate::content_vectorizer::SearchMode>() {
                Ok(mode) => vectorizer.set_default_mode(mode),
                Err(e) => warn!("Ignoring search_mode: {}", e),
            }
        }
    }
}
