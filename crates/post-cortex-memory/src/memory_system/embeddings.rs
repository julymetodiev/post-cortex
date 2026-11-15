use std::sync::Arc;
use std::sync::atomic::Ordering;
use tokio_util::sync::CancellationToken;

use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::content_vectorizer::{ContentVectorizer, ContentVectorizerConfig};

use crate::memory_system::system::MemoryRuntime;
use crate::memory_system::{RuntimeError, SessionHandle};

/// What [`MemoryRuntime::vectorize_and_commit`] embeds.
#[derive(Debug, Clone, Copy)]
pub enum VectorizeTarget<'a> {
    /// These records (by entry id), embedded together.
    Updates(&'a [Uuid]),
    /// Every record and entity of the session that lacks a vector.
    Session,
}

impl VectorizeTarget<'_> {
    /// For error messages: the record when there is one, else a count.
    fn describe(&self) -> String {
        match self {
            Self::Updates([id]) => format!("record {id}"),
            Self::Updates(ids) => format!("{} records", ids.len()),
            Self::Session => "session".to_string(),
        }
    }
}

// Retry configuration constants
const MAX_VECTORIZER_INIT_RETRIES: u32 = 10;

impl MemoryRuntime {
    /// The one "embed, then commit" sequence for a session's records:
    /// vectorize `target` from a snapshot of `handle`, then, when vectors
    /// were added, undo them if the session was deleted meanwhile (so they
    /// do not outlive it) or queue the one persist that stores the
    /// vectorized ids (one per call, however many records it embedded).
    /// Counts as derived work while it runs. Returns how many vectors were
    /// added (0 when the session was deleted).
    pub(crate) async fn vectorize_and_commit(
        &self,
        vectorizer: &Arc<ContentVectorizer>,
        session_id: Uuid,
        handle: &SessionHandle,
        target: VectorizeTarget<'_>,
    ) -> Result<usize, RuntimeError> {
        let _job = self.derived_work.begin(session_id);
        // Owned snapshot: an ArcSwap guard must not live across the
        // embedding await.
        let session = handle.load_full();
        let added = match target {
            VectorizeTarget::Updates(update_ids) => {
                vectorizer
                    .vectorize_updates(&session, handle.vectorized(), update_ids)
                    .await
            }
            VectorizeTarget::Session => {
                vectorizer
                    .vectorize_session(&session, handle.vectorized())
                    .await
            }
        }
        .inspect_err(|e| {
            warn!(
                "Embedding {} of session {session_id} failed: {e}",
                target.describe()
            );
        })?;
        if added == 0 {
            return Ok(0);
        }
        if self.persister.is_deleted(session_id, handle) {
            // Deleted while embedding: the vectors must not outlive it.
            match target {
                VectorizeTarget::Updates(update_ids) => {
                    crate::memory_system::update_vectors::undo_vectorization(
                        vectorizer, session_id, update_ids,
                    )
                    .await;
                }
                VectorizeTarget::Session => {
                    if let Err(e) = vectorizer.remove_session_vectors(session_id).await {
                        warn!("Vectors of deleted session {session_id} kept: {e:#}");
                    }
                }
            }
            debug!("Dropped {added} vector(s) of deleted session {session_id}");
            return Ok(0);
        }
        // Stores the vectorized ids with the session.
        self.persister.persist_nowait(handle, vec![]);
        Ok(added)
    }

    /// Embed the records of stored sessions that have no vectors (and the
    /// entities whose vectors are missing or out of date), e.g. records
    /// whose queued embedding was lost when the process stopped. Sessions
    /// whose records all have vectors are read from storage without being
    /// cached. Idempotent: a record that has a vector is never embedded
    /// again. Once `stop` is cancelled it ends after the session it is on,
    /// so no query or commit is cut off half way. Returns how many vectors
    /// were added.
    ///
    /// # Errors
    /// Returns an error if the vectorizer cannot be initialised or the
    /// sessions cannot be listed.
    pub async fn vectorize_missing_records(
        &self,
        stop: &CancellationToken,
    ) -> Result<usize, RuntimeError> {
        let vectorizer = self.ensure_vectorizer_initialized().await?;
        let mut added = 0;
        for session_id in self.list_sessions().await? {
            if stop.is_cancelled() {
                debug!("Missing-vector pass stopped after {} vector(s)", added);
                break;
            }
            let cached = self
                .session_manager
                .sessions
                .get(&session_id)
                .map(|h| h.load_full());
            let snapshot = match cached {
                Some(s) => s,
                None => match self.session_manager.load_stored(session_id).await {
                    Ok(Some(s)) => Arc::new(s.session),
                    Ok(None) => continue,
                    Err(e) => {
                        warn!(
                            "Session {} not checked for missing vectors: {}",
                            session_id, e
                        );
                        continue;
                    }
                },
            };
            match vectorizer.has_unembedded_updates(&snapshot).await {
                Ok(true) => {}
                Ok(false) => continue,
                Err(e) => {
                    warn!(
                        "Session {} not checked for missing vectors: {:#}",
                        session_id, e
                    );
                    continue;
                }
            }
            let handle = match self.get_session(session_id).await {
                Ok(h) => h,
                Err(e) => {
                    warn!("Session {} not re-embedded: {}", session_id, e);
                    continue;
                }
            };
            match self
                .vectorize_and_commit(&vectorizer, session_id, &handle, VectorizeTarget::Session)
                .await
            {
                Ok(0) => {}
                Ok(n) => {
                    debug!("Embedded {} missing vector(s) of session {}", n, session_id);
                    added += n;
                }
                Err(e) => warn!("Session {} not re-embedded: {}", session_id, e),
            }
        }
        Ok(added)
    }

    /// Lazy-initialize content vectorizer on first use with retry mechanism
    pub(crate) async fn ensure_vectorizer_initialized(
        &self,
    ) -> Result<Arc<ContentVectorizer>, RuntimeError> {
        // Check if already initialized — fast path, no counter increment
        if let Some(vectorizer) = self.content_vectorizer.get() {
            return Ok(Arc::clone(vectorizer));
        }

        // Note: We don't increment the counter here because concurrent callers
        // would burn through the limit. The counter is incremented only on actual
        // initialization failure inside the closure.
        let attempt = self
            .embedding_config_holder
            .init_attempt_count
            .load(Ordering::Relaxed)
            + 1;

        // Check if we've exceeded max retries (set by actual failures, not callers)
        if attempt > MAX_VECTORIZER_INIT_RETRIES as u64 + 1 {
            return Err(RuntimeError::VectorizerGaveUp {
                attempts: attempt - 1,
            });
        }

        info!(
            "Lazy-initializing content vectorizer (attempt {}/{})...",
            attempt,
            MAX_VECTORIZER_INIT_RETRIES + 1
        );

        // Try to initialize with retry logic
        let result: Result<&Arc<ContentVectorizer>, RuntimeError> = self
            .content_vectorizer
            .get_or_try_init(|| async {
                let (Some(engine), Some(index)) = (self.engine(), self.index()) else {
                    return Err(RuntimeError::EmbeddingsNotConfigured);
                };
                let vectorizer_config = ContentVectorizerConfig {
                    enable_cross_session_search: self
                        .embedding_config_holder
                        .cross_session_search_enabled,
                    ..Default::default()
                };
                // The model loads on its first batch: an empty one loads it.
                engine.embed(Vec::new()).await?;
                let mut vectorizer = ContentVectorizer::new(
                    vectorizer_config,
                    Arc::clone(engine),
                    Arc::clone(index),
                );

                self.attach_reranker(&mut vectorizer).await;

                // The stored vectors are what search finds: an index that
                // cannot open (or holds only another model's vectors) fails
                // the start, so the next call retries instead of searching
                // an empty index.
                let stats = index.stats().await?;
                let model = engine.model();
                if stats.dimension != model.dimension {
                    return Err(RuntimeError::DimensionMismatch {
                        engine: model.dimension,
                        index: stats.dimension,
                    });
                }
                vectorizer.set_load_report(stats.load.clone());
                if stats.load.loaded > 0 {
                    info!(
                        "Loaded {} persisted embeddings from storage during initialization",
                        stats.load.loaded
                    );
                }

                Ok(Arc::new(vectorizer))
            })
            .await;

        match result {
            Ok(vectorizer) => {
                info!(
                    "Content vectorizer initialized successfully on attempt {}",
                    attempt
                );
                // Clear any previous error
                *self.embedding_config_holder.last_init_error.write() = None;

                Ok(Arc::clone(vectorizer))
            }
            Err(e) => {
                // Increment counter only on actual failure (not on concurrent caller contention)
                let real_attempt = self
                    .embedding_config_holder
                    .init_attempt_count
                    .fetch_add(1, Ordering::Relaxed)
                    + 1;
                // Store the error for diagnostics
                *self.embedding_config_holder.last_init_error.write() = Some(e.to_string());
                error!(
                    "Vectorizer initialization failed on attempt {}: {}",
                    real_attempt, e
                );
                Err(e)
            }
        }
    }

    /// Vectorize a session's updates and entities (requires embeddings
    /// feature). Searches call this when a session lacks update vectors.
    pub(crate) async fn vectorize_session_updates(
        &self,
        session_id: Uuid,
    ) -> Result<usize, RuntimeError> {
        let vectorizer = self.ensure_vectorizer_initialized().await?;
        let handle = self.get_session(session_id).await?;
        let count = self
            .vectorize_and_commit(&vectorizer, session_id, &handle, VectorizeTarget::Session)
            .await?;
        debug!("Vectorized {} items for session {}", count, session_id);
        Ok(count)
    }
}

#[cfg(test)]
mod tests;
