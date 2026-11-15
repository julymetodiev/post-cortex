// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Admin's vectorization passes: warm-up, one session, every session's
//! records or messages, the missing-record pass, and the vector counts.

use std::sync::atomic::{AtomicUsize, Ordering};
use tokio_util::sync::CancellationToken;

use futures::stream::{self, StreamExt};
use tracing::{info, warn};
use uuid::Uuid;

use crate::capabilities::admin::{Admin, AdminError, PassCounts};
use crate::content_vectorizer::VectorizationStats;
use crate::memory_system::{RuntimeError, VectorizeTarget};

/// Sessions vectorized at a time by [`Admin::vectorize_all_sessions`].
const MAX_PARALLEL_VECTORIZATION: usize = 4;

impl Admin {
    /// Load the embedding model and the persisted vectors now instead of on
    /// the first search (the lazy path costs seconds, which made the first
    /// query after a restart time out in short-deadline clients), then embed
    /// the records stored without vectors (e.g. whose queued embedding was
    /// lost when the process stopped). The memory module's start runs it
    /// in the background; search never runs the missing-record pass. Once
    /// `stop` is cancelled the pass ends after the session it is on.
    ///
    /// # Errors
    /// The model or the stored vectors could not be loaded.
    pub async fn warm_up(&self, stop: &CancellationToken) -> Result<(), AdminError> {
        if !self.rt.config.enable_embeddings {
            return Ok(());
        }
        self.rt
            .ensure_vectorizer_initialized()
            .await
            .map_err(AdminError::from)?;
        if self.rt.config.auto_vectorize_on_update {
            match self.rt.vectorize_missing_records(stop).await {
                Ok(0) => {}
                Ok(n) => info!("Embedded {} vector(s) of records stored without them", n),
                Err(e) => warn!("Records without vectors not embedded: {}", e),
            }
        }
        Ok(())
    }

    /// Vectorize a session's content: its records and entities, and the
    /// captured messages without vectors (an embedding that failed on
    /// append); returns how many vectors were added.
    ///
    /// # Errors
    /// The session does not exist or embedding failed.
    pub async fn vectorize_session(&self, session_id: Uuid) -> Result<usize, AdminError> {
        let records = self
            .rt
            .vectorize_session_updates(session_id)
            .await
            .map_err(AdminError::from)?;
        let messages = self
            .rt
            .vectorize_session_messages(session_id)
            .await
            .map_err(AdminError::from)?;
        Ok(records + messages)
    }

    /// Vectorize every stored session's records and entities, a few
    /// sessions at a time (sessions deleted meanwhile keep no vectors).
    ///
    /// # Errors
    /// The vectorizer could not start or the sessions not be listed.
    pub async fn vectorize_all_sessions(&self) -> Result<PassCounts, AdminError> {
        let rt = &self.rt;
        info!("Starting full vectorization of all sessions");
        let start_time = std::time::Instant::now();
        let vectorizer = rt
            .ensure_vectorizer_initialized()
            .await
            .map_err(AdminError::from)?;
        let session_ids = rt.list_sessions().await.map_err(AdminError::from)?;
        let total_sessions = session_ids.len();
        if total_sessions == 0 {
            info!("No sessions found to vectorize");
            return Ok((0, 0, 0));
        }
        info!(
            "Found {} sessions to vectorize (max {} at a time)",
            total_sessions, MAX_PARALLEL_VECTORIZATION
        );

        let processed = AtomicUsize::new(0);
        let outcomes: Vec<Result<usize, RuntimeError>> = stream::iter(session_ids)
            .map(|session_id| {
                let (vectorizer, processed) = (&vectorizer, &processed);
                async move {
                    let result = match rt.get_session(session_id).await {
                        Ok(handle) => {
                            rt.vectorize_and_commit(
                                vectorizer,
                                session_id,
                                &handle,
                                VectorizeTarget::Session,
                            )
                            .await
                        }
                        Err(e) => Err(e),
                    };
                    let n = processed.fetch_add(1, Ordering::Relaxed) + 1;
                    match &result {
                        Ok(count) => info!(
                            "[{n}/{total_sessions}] Vectorized {count} items for session {session_id}"
                        ),
                        Err(e) => warn!(
                            "[{n}/{total_sessions}] Failed to vectorize session {session_id}: {e}"
                        ),
                    }
                    result
                }
            })
            .buffer_unordered(MAX_PARALLEL_VECTORIZATION)
            .collect()
            .await;

        vectorizer.clear_query_cache();
        let total: usize = outcomes.iter().filter_map(|r| r.as_ref().ok()).sum();
        let failed = outcomes.iter().filter(|r| r.is_err()).count();
        let success = outcomes.len() - failed;
        info!(
            "Bulk vectorization complete in {:.2}s: {} total items across {} successful sessions ({} failed)",
            start_time.elapsed().as_secs_f64(),
            total,
            success,
            failed
        );
        Ok((total, success, failed))
    }

    /// Repair the message vectors of every session, one at a time (`pcx
    /// vectorize-all --messages`): returns the vectors added, the sessions
    /// done and the sessions that failed (logged).
    ///
    /// # Errors
    /// The sessions could not be listed.
    pub async fn vectorize_all_messages(&self) -> Result<PassCounts, AdminError> {
        let (mut added, mut done, mut failed) = (0, 0, 0);
        for session_id in self.rt.list_sessions().await.map_err(AdminError::from)? {
            match self.rt.vectorize_session_messages(session_id).await {
                Ok(count) => {
                    added += count;
                    done += 1;
                }
                Err(e) => {
                    warn!("Message vectors of session {session_id} not repaired: {e}");
                    failed += 1;
                }
            }
        }
        Ok((added, done, failed))
    }

    /// Size of the search index.
    ///
    /// # Errors
    /// The vectorizer is not initialised yet, or the index could not be
    /// read.
    pub async fn vectorization_stats(&self) -> Result<VectorizationStats, AdminError> {
        match self.rt.content_vectorizer.get() {
            Some(vectorizer) => Ok(vectorizer.get_vectorization_stats().await?),
            None => Err(AdminError::EmbeddingsNotStarted),
        }
    }
}
