// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Records: write, bulk write, validate (dry run), list, delete, forget,
//! purge, and the change feed
//! (ARCHITECTURE section 4). The one write validation lives here
//! ([`validate`]); a write is built into its record once and committed with
//! its graph merge in one swap, and its embedding is handed to the bounded
//! pipeline ([`derived`]).

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use post_cortex_core::redact::Redactor;
use post_cortex_core::services::{
    BulkUpdateContextRequest, BulkUpdateContextResponse, UpdateContextRequest,
    UpdateContextResponse,
};
use post_cortex_core::session::active_session::ActiveSession;
use uuid::Uuid;

use crate::capabilities::derived::DerivedQueue;
use crate::memory_system::{MemoryRuntime, SessionHandle};
use crate::pipeline::Embed;

mod changes;
mod commit;
mod list;
mod validate;
#[cfg(test)]
pub(crate) use validate::tests::good_request;

pub use list::{ListRecords, RecordPage};

/// Why a record operation failed.
#[derive(Debug, thiserror::Error)]
pub enum RecordError {
    /// The request is malformed (empty content, dangling relation, unknown
    /// superseded entry, ...).
    #[error("Invalid argument: {0}")]
    InvalidArgument(String),
    /// The session does not exist (writes never create one).
    #[error("Session {0} not found")]
    SessionNotFound(Uuid),
    /// The circuit breaker is open.
    #[error("{0}")]
    Unavailable(String),
    /// Storage or the session swap failed.
    #[error("{0}")]
    Storage(#[from] crate::memory_system::RuntimeError),
    /// An item of a bulk write failed; the items before it were written.
    #[error("updates[{index}]: {error}")]
    BulkItem {
        /// Position of the item in the request.
        index: usize,
        /// Why it failed.
        error: Box<RecordError>,
    },
}

/// The one mapping of a storage port error to this capability's error.
fn storage(e: post_cortex_ports::PortError) -> RecordError {
    RecordError::Storage(e.into())
}

/// The Records capability service.
#[derive(Clone)]
pub struct Records {
    rt: Arc<MemoryRuntime>,
    derived: DerivedQueue,
    redactor: Arc<Redactor>,
}

impl Records {
    pub(crate) fn new(
        rt: Arc<MemoryRuntime>,
        derived: DerivedQueue,
        redactor: Arc<Redactor>,
    ) -> Self {
        Self {
            rt,
            derived,
            redactor,
        }
    }

    /// The cached handle of an existing session (loaded if needed); the
    /// record operations in this module's files share it.
    async fn handle(&self, session_id: Uuid) -> Result<SessionHandle, RecordError> {
        self.rt
            .session_manager
            .get_session(session_id)
            .await
            .map_err(RecordError::Storage)?
            .ok_or(RecordError::SessionNotFound(session_id))
    }

    /// Write one record to an existing session: redacted, validated, stored
    /// with its graph merge, then embedded in the background. Returns once
    /// the storage write finished (`durable` says whether it succeeded).
    ///
    /// # Errors
    /// `InvalidArgument` for a malformed request, `SessionNotFound`,
    /// `Unavailable` while the circuit breaker is open.
    #[tracing::instrument(
        skip(self, req),
        fields(
            session_id = %req.session_id,
            interaction_type = ?req.interaction_type,
            entities = req.entities.len(),
            relations = req.relations.len(),
        ),
        name = "post_cortex.update_context",
    )]
    pub async fn write(
        &self,
        mut req: UpdateContextRequest,
    ) -> Result<UpdateContextResponse, RecordError> {
        self.prepare(&mut req).await?;
        let session_id = req.session_id;
        // Waits for the storage write, so `durable` reports what happened.
        let committed = self.commit(session_id, validate::record(req), true).await?;
        self.submit_derived(session_id, committed.entry_id).await;
        Ok(UpdateContextResponse {
            entry_id: committed.entry_id,
            session_id,
            persisted_at: Utc::now(),
            durable: committed.durable,
        })
    }

    /// Run every check [`Self::write`] runs (redaction, validation, the
    /// session lookup and the references) and store nothing: a dry run.
    ///
    /// # Errors
    /// As [`Self::write`].
    pub async fn validate(&self, mut req: UpdateContextRequest) -> Result<(), RecordError> {
        self.prepare(&mut req).await
    }

    /// Write several records to one session. The whole batch is validated
    /// first, so a bad item writes nothing; then the records are stored in
    /// order and the call returns once the last one is written. The entry
    /// ids are in input order (`entry_ids[i]` is `updates[i]`).
    ///
    /// # Errors
    /// As [`Self::write`]; a failing item is named by its index.
    #[tracing::instrument(
        skip(self, req),
        fields(session_id = %req.session_id, batch_size = req.updates.len()),
        name = "post_cortex.bulk_update_context",
    )]
    pub async fn bulk_write(
        &self,
        mut req: BulkUpdateContextRequest,
    ) -> Result<BulkUpdateContextResponse, RecordError> {
        self.prepare_bulk(&mut req).await?;
        let mut entry_ids = Vec::with_capacity(req.updates.len());
        let mut durable = false;
        let last = req.updates.len().saturating_sub(1);
        for (i, item) in req.updates.into_iter().enumerate() {
            let session_id = item.session_id;
            // Only the last item waits: session writes are ordered and
            // coalesced, so the write it waits for covers every row before it.
            let committed = self
                .commit(session_id, validate::record(item), i == last)
                .await
                .map_err(|e| RecordError::BulkItem {
                    index: i,
                    error: Box::new(e),
                })?;
            durable = committed.durable;
            entry_ids.push(committed.entry_id);
            self.submit_derived(session_id, committed.entry_id).await;
        }

        Ok(BulkUpdateContextResponse {
            entry_ids,
            persisted_at: Utc::now(),
            durable,
        })
    }

    /// Run every check [`Self::bulk_write`] runs and store nothing.
    ///
    /// # Errors
    /// As [`Self::bulk_write`].
    pub async fn validate_bulk(
        &self,
        mut req: BulkUpdateContextRequest,
    ) -> Result<(), RecordError> {
        self.prepare_bulk(&mut req).await
    }

    /// Wait until the derived work of every write so far (embeddings) is
    /// done, at most `limit`; returns whether it is.
    pub async fn wait_idle(&self, limit: Duration) -> bool {
        self.rt.derived_work.wait_idle(limit).await
    }

    /// Redact and validate one write against its existing session.
    async fn prepare(&self, req: &mut UpdateContextRequest) -> Result<(), RecordError> {
        validate::log_redactions(req.session_id, &self.redactor.redact_request(req));
        validate::request(req)?;
        let session = self.require_session(req.session_id).await?;
        self.check_references(req, &session).await
    }

    /// Redact and validate every item of a bulk write; the first bad item
    /// fails the batch, named by its index.
    async fn prepare_bulk(&self, req: &mut BulkUpdateContextRequest) -> Result<(), RecordError> {
        for item in &mut req.updates {
            validate::log_redactions(item.session_id, &self.redactor.redact_request(item));
        }
        let session = self.require_session(req.session_id).await?;
        for (i, item) in req.updates.iter().enumerate() {
            if item.session_id != req.session_id {
                return Err(RecordError::InvalidArgument(format!(
                    "bulk_update_context: updates[{i}].session_id {} does not match request session_id {}",
                    item.session_id, req.session_id
                )));
            }
            let prefixed = |e| match e {
                RecordError::InvalidArgument(msg) => {
                    RecordError::InvalidArgument(format!("updates[{i}]: {msg}"))
                }
                other => other,
            };
            validate::request(item).map_err(prefixed)?;
            self.check_references(item, &session)
                .await
                .map_err(prefixed)?;
        }
        Ok(())
    }

    /// The current snapshot of an existing session.
    async fn require_session(&self, session_id: Uuid) -> Result<Arc<ActiveSession>, RecordError> {
        Ok(self.handle(session_id).await?.load_full())
    }

    /// The references of a write resolve in its session: `derived_from`
    /// names stored messages, `supersedes` names its records.
    async fn check_references(
        &self,
        req: &UpdateContextRequest,
        session: &ActiveSession,
    ) -> Result<(), RecordError> {
        if let Some(seqs) = validate::derived_from(req)? {
            self.check_messages(req.session_id, &seqs).await?;
        }
        validate::meta(req, session)
    }

    /// Every message position in `seqs` is stored in the session.
    async fn check_messages(&self, session_id: Uuid, seqs: &[u64]) -> Result<(), RecordError> {
        let (first, last) = (
            seqs.iter().min().copied().unwrap_or(0),
            seqs.iter().max().copied().unwrap_or(0),
        );
        let stored: std::collections::HashSet<u64> = self
            .rt
            .stores
            .messages
            .load_messages(session_id, first..last + 1)
            .await
            .map_err(storage)?
            .into_iter()
            .map(|m| m.seq)
            .collect();
        match seqs.iter().find(|seq| !stored.contains(seq)) {
            Some(missing) => Err(RecordError::InvalidArgument(format!(
                "derived_from: message:{missing} is not in session {session_id}"
            ))),
            None => Ok(()),
        }
    }

    async fn submit_derived(&self, session_id: Uuid, entry_id: Uuid) {
        self.derived
            .submit(session_id, Embed::Record(entry_id))
            .await;
    }
}

#[cfg(test)]
mod tests;
