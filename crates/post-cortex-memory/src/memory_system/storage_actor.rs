use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc::{Receiver, Sender, channel};
use tokio::sync::{Semaphore, oneshot};
use tokio::time::timeout;
use tracing::{error, info, trace};
use uuid::Uuid;

use post_cortex_core::session::active_session::ActiveSession;
use post_cortex_proto::pb::{CascadeInvalidateReport, FreshnessEntry, SourceReference, SymbolId};
use post_cortex_storage::traits::SessionSummary;

use super::config::OperationType;
use super::persister::{PersistSink, SessionHandle, SessionPersister, StorageSink};

/// Requests queued for the actor before senders wait for room.
const REQUEST_QUEUE_CAPACITY: usize = 1024;
/// Requests the actor runs at the same time.
const MAX_CONCURRENT_REQUESTS: usize = 32;

/// Storage actor: runs storage requests off the caller's task.
///
/// Requests are handled concurrently (up to `MAX_CONCURRENT_REQUESTS`);
/// writes that touch one session are ordered by the per-session writer in
/// `SessionPersister`, which also coalesces background session persists.
pub struct StorageActor {
    receiver: Receiver<StorageMessage>,
    ctx: Arc<ActorContext>,
}

/// State shared by the actor's request tasks.
struct ActorContext {
    storage: Arc<dyn post_cortex_storage::traits::GraphStorage>,
    persister: Arc<SessionPersister>,
}

/// Handle for communicating with storage actor
#[derive(Clone)]
pub struct StorageActorHandle {
    sender: Sender<StorageMessage>,
    persister: Arc<SessionPersister>,
}

/// Messages for storage actor
#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub enum StorageMessage {
    /// Load a session by its ID.
    LoadSession {
        /// ID of the session to load.
        session_id: Uuid,
        /// One-shot channel for returning the result.
        response_tx: oneshot::Sender<Result<Option<ActiveSession>, String>>,
    },
    /// Save a complete session to storage.
    SaveSession {
        /// Session to persist.
        session: Box<ActiveSession>,
        /// One-shot channel for returning the result.
        response_tx: oneshot::Sender<Result<(), String>>,
    },
    /// Delete a session by its ID.
    DeleteSession {
        /// ID of the session to delete.
        session_id: Uuid,
        /// The session's in-memory handle, if loaded: persists still queued
        /// or in flight for it are dropped instead of writing it back.
        live: Option<SessionHandle>,
        /// One-shot channel for returning whether the session existed.
        response_tx: oneshot::Sender<Result<bool, String>>,
    },
    /// Clear all entities and relationships for a session.
    ClearSessionEntities {
        /// ID of the session whose entities should be cleared.
        session_id: Uuid,
        /// One-shot channel for returning the result.
        response_tx: oneshot::Sender<Result<(), String>>,
    },
    /// Delete a specific entity from a session.
    DeleteEntity {
        /// Session that owns the entity.
        session_id: Uuid,
        /// Name of the entity to delete.
        entity_name: String,
        /// One-shot channel for returning the result.
        response_tx: oneshot::Sender<Result<(), String>>,
    },
    /// List all stored session IDs.
    ListSessions {
        /// One-shot channel for returning the session list.
        response_tx: oneshot::Sender<Result<Vec<Uuid>, String>>,
    },
    /// List every stored session's metadata without loading the sessions.
    ListSessionSummaries {
        /// One-shot channel for returning the summaries.
        response_tx: oneshot::Sender<Result<Vec<SessionSummary>, String>>,
    },
    /// Save a session checkpoint.
    SaveCheckpoint {
        /// Checkpoint data to persist.
        checkpoint: post_cortex_storage::rocksdb_storage::SessionCheckpoint,
        /// One-shot channel for returning the result.
        response_tx: oneshot::Sender<Result<(), String>>,
    },
    /// Load a session checkpoint by ID.
    LoadCheckpoint {
        /// ID of the checkpoint to load.
        checkpoint_id: Uuid,
        /// One-shot channel for returning the checkpoint.
        response_tx: oneshot::Sender<
            Result<post_cortex_storage::rocksdb_storage::SessionCheckpoint, String>,
        >,
    },
    /// Save or update workspace metadata.
    SaveWorkspaceMetadata {
        /// Workspace ID.
        workspace_id: Uuid,
        /// Human-readable workspace name.
        name: String,
        /// Workspace description.
        description: String,
        /// Sessions belonging to this workspace.
        session_ids: Vec<Uuid>,
        /// One-shot channel for returning the result.
        response_tx: oneshot::Sender<Result<(), String>>,
    },
    /// Delete a workspace by its ID.
    DeleteWorkspace {
        /// ID of the workspace to delete.
        workspace_id: Uuid,
        /// One-shot channel for returning the result.
        response_tx: oneshot::Sender<Result<(), String>>,
    },
    /// Add a session to a workspace with a given role.
    AddSessionToWorkspace {
        /// Target workspace.
        workspace_id: Uuid,
        /// Session to add.
        session_id: Uuid,
        /// Role assigned to the session within the workspace.
        role: post_cortex_core::workspace::SessionRole,
        /// One-shot channel for returning the result.
        response_tx: oneshot::Sender<Result<(), String>>,
    },
    /// Remove a session from a workspace.
    RemoveSessionFromWorkspace {
        /// Workspace to remove from.
        workspace_id: Uuid,
        /// Session to remove.
        session_id: Uuid,
        /// One-shot channel for returning the result.
        response_tx: oneshot::Sender<Result<(), String>>,
    },
    /// List all stored workspaces.
    ListAllWorkspaces {
        /// One-shot channel for returning the workspace list.
        response_tx: oneshot::Sender<
            Result<Vec<post_cortex_storage::rocksdb_storage::StoredWorkspace>, String>,
        >,
    },
    /// Batch-save multiple context updates for a session.
    BatchSaveUpdates {
        /// Target session.
        session_id: Uuid,
        /// Updates to persist.
        updates: Vec<post_cortex_core::core::context_update::ContextUpdate>,
        /// One-shot channel for returning the result.
        response_tx: oneshot::Sender<Result<(), String>>,
    },
    /// Register a source reference for freshness tracking.
    RegisterSource {
        /// Session that owns the source.
        session_id: Uuid,
        /// Source reference to register.
        source_ref: SourceReference,
        /// One-shot channel for returning the result.
        response_tx: oneshot::Sender<Result<(), String>>,
    },
    /// Check freshness of a single cache entry.
    CheckFreshness {
        /// Cache entry identifier.
        entry_id: String,
        /// Current file content hash.
        file_hash: Vec<u8>,
        /// Current AST hash, if available.
        ast_hash: Option<Vec<u8>>,
        /// Symbol name for semantic-level checks.
        symbol_name: Option<String>,
        /// One-shot channel for returning the freshness result.
        response_tx: oneshot::Sender<Result<FreshnessEntry, String>>,
    },
    /// Check freshness of multiple cache entries in one call.
    CheckFreshnessBatch {
        /// Batch of (entry_id, file_hash, ast_hash, symbol_name) tuples.
        entries: Vec<(String, Vec<u8>, Option<Vec<u8>>, Option<String>)>,
        /// One-shot channel for returning the batch results.
        response_tx: oneshot::Sender<Result<Vec<FreshnessEntry>, String>>,
    },
    /// Invalidate all cache entries derived from a source file.
    InvalidateSource {
        /// Path of the source file to invalidate.
        file_path: String,
        /// One-shot channel for returning the number of invalidated entries.
        response_tx: oneshot::Sender<Result<u32, String>>,
    },
    /// Register dependency edges from one symbol to a set of target symbols.
    RegisterSymbolDependencies {
        /// Source symbol.
        from: SymbolId,
        /// Target symbols that `from` depends on.
        to_symbols: Vec<SymbolId>,
        /// One-shot channel for returning the number of edges created.
        response_tx: oneshot::Sender<Result<u32, String>>,
    },
    /// Cascade invalidation from a changed symbol to its transitive dependents.
    CascadeInvalidate {
        /// Symbol that changed.
        changed: SymbolId,
        /// New AST hash for the changed symbol.
        new_ast_hash: Vec<u8>,
        /// Maximum traversal depth for the cascade.
        max_depth: u32,
        /// One-shot channel for returning the cascade report.
        response_tx: oneshot::Sender<Result<CascadeInvalidateReport, String>>,
    },
    /// Retrieve stale entries associated with a source file.
    GetStaleEntriesBySource {
        /// Source file path to query.
        file_path: String,
        /// One-shot channel for returning the stale entries.
        response_tx:
            oneshot::Sender<Result<Vec<post_cortex_storage::traits::StaleEntryInfo>, String>>,
    },
    /// Shut down the storage actor loop.
    Shutdown,
}

impl StorageActorHandle {
    /// Send a request message and await its `Result<T, String>` response with a timeout.
    ///
    /// `op_name` is only invoked on the timeout error path — keeps allocations
    /// off the hot success path.
    async fn send_request<T, B, N>(
        &self,
        op_type: OperationType,
        op_name: N,
        build_msg: B,
    ) -> Result<T, String>
    where
        B: FnOnce(oneshot::Sender<Result<T, String>>) -> StorageMessage,
        N: FnOnce() -> String,
    {
        let (response_tx, response_rx) = oneshot::channel::<Result<T, String>>();
        let timeout_duration = op_type.timeout();
        let exchange = async {
            self.sender
                .send(build_msg(response_tx))
                .await
                .map_err(|_| "Storage actor unavailable".to_string())?;
            response_rx
                .await
                .map_err(|_| "Storage actor response channel closed".to_string())?
        };
        match tokio::time::timeout(timeout_duration, exchange).await {
            Ok(result) => result,
            Err(_) => Err(format!(
                "{} timed out after {}s",
                op_name(),
                timeout_duration.as_secs()
            )),
        }
    }

    /// Load a session from storage by ID. `Ok(None)` means storage holds
    /// no such session; any other failure is an error, never `None`, so
    /// callers cannot mistake a read failure for a missing session.
    pub async fn load_session(&self, session_id: Uuid) -> Result<Option<ActiveSession>, String> {
        self.send_request(
            OperationType::Fast,
            || format!("LoadSession {session_id}"),
            |response_tx| StorageMessage::LoadSession {
                session_id,
                response_tx,
            },
        )
        .await
    }

    /// Save a complete session to storage.
    pub async fn save_session(&self, session: ActiveSession) -> Result<(), String> {
        let session_id = session.id();
        self.send_request(
            OperationType::Medium,
            || format!("SaveSession {session_id}"),
            |response_tx| StorageMessage::SaveSession {
                session: Box::new(session),
                response_tx,
            },
        )
        .await
    }

    /// Clear all entities and relationships for a session from storage.
    async fn clear_session_entities(&self, session_id: Uuid) -> Result<(), String> {
        self.send_request(
            OperationType::Medium,
            || format!("ClearSessionEntities {session_id}"),
            |response_tx| StorageMessage::ClearSessionEntities {
                session_id,
                response_tx,
            },
        )
        .await
    }

    /// Rebuild entity graph for a session by clearing it and replaying all stored updates.
    /// Returns (entities_before, entities_after) counts.
    pub async fn rebuild_entity_graph(&self, session_id: Uuid) -> Result<(usize, usize), String> {
        let mut session = self
            .load_session(session_id)
            .await?
            .ok_or_else(|| format!("Session {session_id} not found"))?;

        let stats = session
            .rebuild_entity_graph_from_updates()
            .await
            .map_err(|e| format!("Rebuild failed: {e}"))?;

        self.clear_session_entities(session_id).await?;
        self.save_session(session).await?;

        Ok(stats)
    }

    /// Queue a background write of `session`'s latest version plus
    /// `updates` and return at once. Writes of one session are ordered and
    /// coalesced: requests queued while a write runs are merged into one
    /// blob write that snapshots `session` when it starts, so the stored
    /// blob always ends at the newest version. Failures are logged.
    pub fn persist_session_nowait(
        &self,
        session: &SessionHandle,
        updates: Vec<post_cortex_core::core::context_update::ContextUpdate>,
    ) {
        let session_id = session.load().id();
        self.persister
            .enqueue(session_id, session, updates, vec![], None);
    }

    /// Like [`Self::persist_session_nowait`], and delete the rows of the
    /// `removed` updates. Saves never delete an update row otherwise, so
    /// every path that removes updates from a session must name them here.
    pub fn persist_session_removals_nowait(&self, session: &SessionHandle, removed: Vec<Uuid>) {
        let session_id = session.load().id();
        self.persister
            .enqueue(session_id, session, vec![], removed, None);
    }

    /// Register a handle just loaded or created as the session's current
    /// one. Persists through older handles of the session then write the
    /// newest one instead of their own (older) snapshot.
    pub fn register_session_handle(&self, session_id: Uuid, session: &SessionHandle) {
        self.persister.register(session_id, session);
    }

    /// Whether the session has writes queued or running, or its last write
    /// failed. Such a session must be flushed before it leaves memory.
    pub fn has_unflushed(&self, session_id: Uuid) -> bool {
        self.persister.has_unflushed(session_id)
    }

    /// Wait until the writes queued for the session so far are done.
    pub async fn wait_persisted(&self, session_id: Uuid) {
        self.persister.wait_idle(session_id).await;
    }

    /// Write every queued session persist and retry failed ones, waiting
    /// at most `limit`. Returns how many sessions could not be written.
    /// Call on graceful shutdown.
    pub async fn flush_all(&self, limit: Duration) -> usize {
        self.persister.flush_all(limit).await
    }

    /// Like [`Self::persist_session_nowait`], but wait until a write that
    /// covers this request (blob and `updates`' rows) has finished.
    pub async fn persist_session_and_wait(
        &self,
        session: &SessionHandle,
        updates: Vec<post_cortex_core::core::context_update::ContextUpdate>,
    ) -> Result<(), String> {
        let session_id = session.load().id();
        let (tx, rx) = oneshot::channel();
        self.persister
            .enqueue(session_id, session, updates, vec![], Some(tx));
        let limit = OperationType::Medium.timeout();
        match timeout(limit, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err("Session persister dropped the request".to_string()),
            Err(_) => Err(format!(
                "Persist of session {session_id} timed out after {}s",
                limit.as_secs()
            )),
        }
    }

    /// Whether `session` is a handle of a session deleted via
    /// [`Self::delete_session`]. Work finished on such a handle after the
    /// delete must be undone rather than persisted.
    pub fn is_deleted(&self, session_id: Uuid, session: &SessionHandle) -> bool {
        self.persister.is_deleted(session_id, session)
    }

    /// Delete a session from storage by ID. Pass the session's in-memory
    /// handle when it is loaded: queued and in-flight persists of it are
    /// dropped, so the delete cannot be undone by a late write.
    pub async fn delete_session(
        &self,
        session_id: Uuid,
        live: Option<SessionHandle>,
    ) -> Result<bool, String> {
        self.send_request(
            OperationType::Medium,
            || format!("DeleteSession {session_id}"),
            |response_tx| StorageMessage::DeleteSession {
                session_id,
                live,
                response_tx,
            },
        )
        .await
    }

    /// Delete a specific entity from a session.
    pub async fn delete_entity(&self, session_id: Uuid, entity_name: &str) -> Result<(), String> {
        let entity_name = entity_name.to_string();
        let entity_name_for_err = entity_name.clone();
        self.send_request(
            OperationType::Medium,
            || format!("DeleteEntity {session_id}/{entity_name_for_err}"),
            |response_tx| StorageMessage::DeleteEntity {
                session_id,
                entity_name,
                response_tx,
            },
        )
        .await
    }

    /// List all stored session IDs.
    pub async fn list_sessions(&self) -> Result<Vec<Uuid>, String> {
        self.send_request(
            OperationType::Slow,
            || "ListSessions".to_string(),
            |response_tx| StorageMessage::ListSessions { response_tx },
        )
        .await
    }

    /// List every stored session's metadata (no particular order) without
    /// loading the sessions.
    pub async fn list_session_summaries(&self) -> Result<Vec<SessionSummary>, String> {
        self.send_request(
            OperationType::Slow,
            || "ListSessionSummaries".to_string(),
            |response_tx| StorageMessage::ListSessionSummaries { response_tx },
        )
        .await
    }

    /// Save a session checkpoint to storage.
    pub async fn save_checkpoint(
        &self,
        checkpoint: &post_cortex_storage::rocksdb_storage::SessionCheckpoint,
    ) -> Result<(), String> {
        let checkpoint = checkpoint.clone();
        self.send_request(
            OperationType::Medium,
            || "SaveCheckpoint".to_string(),
            |response_tx| StorageMessage::SaveCheckpoint {
                checkpoint,
                response_tx,
            },
        )
        .await
    }

    /// Load a session checkpoint by ID.
    pub async fn load_checkpoint(
        &self,
        checkpoint_id: Uuid,
    ) -> Result<post_cortex_storage::rocksdb_storage::SessionCheckpoint, String> {
        self.send_request(
            OperationType::Fast,
            || format!("LoadCheckpoint {checkpoint_id}"),
            |response_tx| StorageMessage::LoadCheckpoint {
                checkpoint_id,
                response_tx,
            },
        )
        .await
    }

    /// Save or update workspace metadata including its sessions.
    pub async fn save_workspace_metadata(
        &self,
        workspace_id: Uuid,
        name: &str,
        description: &str,
        session_ids: &[Uuid],
    ) -> Result<(), String> {
        let name = name.to_string();
        let description = description.to_string();
        let session_ids = session_ids.to_vec();
        self.send_request(
            OperationType::Medium,
            || "SaveWorkspaceMetadata".to_string(),
            |response_tx| StorageMessage::SaveWorkspaceMetadata {
                workspace_id,
                name,
                description,
                session_ids,
                response_tx,
            },
        )
        .await
    }

    /// List all stored workspaces.
    pub async fn list_all_workspaces(
        &self,
    ) -> Result<Vec<post_cortex_storage::rocksdb_storage::StoredWorkspace>, String> {
        self.send_request(
            OperationType::Slow,
            || "ListAllWorkspaces".to_string(),
            |response_tx| StorageMessage::ListAllWorkspaces { response_tx },
        )
        .await
    }

    /// Delete a workspace by its ID.
    pub async fn delete_workspace(&self, workspace_id: Uuid) -> Result<(), String> {
        self.send_request(
            OperationType::Medium,
            || format!("DeleteWorkspace {workspace_id}"),
            |response_tx| StorageMessage::DeleteWorkspace {
                workspace_id,
                response_tx,
            },
        )
        .await
    }

    /// Add a session to a workspace with a given role.
    pub async fn add_session_to_workspace(
        &self,
        workspace_id: Uuid,
        session_id: Uuid,
        role: post_cortex_core::workspace::SessionRole,
    ) -> Result<(), String> {
        self.send_request(
            OperationType::Fast,
            || "AddSessionToWorkspace".to_string(),
            |response_tx| StorageMessage::AddSessionToWorkspace {
                workspace_id,
                session_id,
                role,
                response_tx,
            },
        )
        .await
    }

    /// Remove a session from a workspace.
    pub async fn remove_session_from_workspace(
        &self,
        workspace_id: Uuid,
        session_id: Uuid,
    ) -> Result<(), String> {
        self.send_request(
            OperationType::Fast,
            || "RemoveSessionFromWorkspace".to_string(),
            |response_tx| StorageMessage::RemoveSessionFromWorkspace {
                workspace_id,
                session_id,
                response_tx,
            },
        )
        .await
    }

    /// Batch-save multiple context updates for a session.
    pub async fn batch_save_updates(
        &self,
        session_id: Uuid,
        updates: Vec<post_cortex_core::core::context_update::ContextUpdate>,
    ) -> Result<(), String> {
        self.send_request(
            OperationType::Medium,
            || format!("BatchSaveUpdates {session_id}"),
            |response_tx| StorageMessage::BatchSaveUpdates {
                session_id,
                updates,
                response_tx,
            },
        )
        .await
    }

    /// Register a source reference for freshness tracking.
    pub async fn register_source(
        &self,
        session_id: Uuid,
        source_ref: SourceReference,
    ) -> Result<(), String> {
        self.send_request(
            OperationType::Medium,
            || "RegisterSource".to_string(),
            |response_tx| StorageMessage::RegisterSource {
                session_id,
                source_ref,
                response_tx,
            },
        )
        .await
    }

    /// Check freshness of a cache entry using file hash only.
    pub async fn check_freshness(
        &self,
        entry_id: String,
        file_hash: Vec<u8>,
    ) -> Result<FreshnessEntry, String> {
        self.check_freshness_semantic(entry_id, file_hash, None, None)
            .await
    }

    /// Check freshness of a cache entry with optional AST hash and symbol name.
    pub async fn check_freshness_semantic(
        &self,
        entry_id: String,
        file_hash: Vec<u8>,
        ast_hash: Option<Vec<u8>>,
        symbol_name: Option<String>,
    ) -> Result<FreshnessEntry, String> {
        let entry_id_for_err = entry_id.clone();
        self.send_request(
            OperationType::Fast,
            || format!("CheckFreshness {entry_id_for_err}"),
            |response_tx| StorageMessage::CheckFreshness {
                entry_id,
                file_hash,
                ast_hash,
                symbol_name,
                response_tx,
            },
        )
        .await
    }

    /// Check freshness of multiple cache entries in a single batch call.
    pub async fn check_freshness_batch(
        &self,
        entries: Vec<(String, Vec<u8>, Option<Vec<u8>>, Option<String>)>,
    ) -> Result<Vec<FreshnessEntry>, String> {
        self.send_request(
            OperationType::Medium,
            || "CheckFreshnessBatch".to_string(),
            |response_tx| StorageMessage::CheckFreshnessBatch {
                entries,
                response_tx,
            },
        )
        .await
    }

    /// Invalidate all cache entries derived from a source file.
    pub async fn invalidate_source(&self, file_path: &str) -> Result<u32, String> {
        let file_path = file_path.to_string();
        self.send_request(
            OperationType::Medium,
            || "InvalidateSource".to_string(),
            |response_tx| StorageMessage::InvalidateSource {
                file_path,
                response_tx,
            },
        )
        .await
        .map_err(|e: String| {
            error!("Storage actor failed to invalidate source: {}", e);
            e
        })
    }

    /// Register dependency edges from one symbol to a set of target symbols.
    pub async fn register_symbol_dependencies(
        &self,
        from: SymbolId,
        to_symbols: Vec<SymbolId>,
    ) -> Result<u32, String> {
        self.send_request(
            OperationType::Medium,
            || "RegisterSymbolDependencies".to_string(),
            |response_tx| StorageMessage::RegisterSymbolDependencies {
                from,
                to_symbols,
                response_tx,
            },
        )
        .await
    }

    /// Cascade invalidation from a changed symbol to its transitive dependents.
    pub async fn cascade_invalidate(
        &self,
        changed: SymbolId,
        new_ast_hash: Vec<u8>,
        max_depth: u32,
    ) -> Result<CascadeInvalidateReport, String> {
        self.send_request(
            OperationType::Medium,
            || "CascadeInvalidate".to_string(),
            |response_tx| StorageMessage::CascadeInvalidate {
                changed,
                new_ast_hash,
                max_depth,
                response_tx,
            },
        )
        .await
    }

    /// Retrieve stale entries associated with a source file.
    pub async fn get_stale_entries_by_source(
        &self,
        file_path: &str,
    ) -> Result<Vec<post_cortex_storage::traits::StaleEntryInfo>, String> {
        let file_path = file_path.to_string();
        self.send_request(
            OperationType::Medium,
            || "GetStaleEntriesBySource".to_string(),
            |response_tx| StorageMessage::GetStaleEntriesBySource {
                file_path,
                response_tx,
            },
        )
        .await
    }
}

impl StorageActor {
    /// Spawn the storage actor on a new tokio task and return a handle for sending messages.
    pub async fn spawn(
        storage: Arc<dyn post_cortex_storage::traits::GraphStorage>,
    ) -> Result<StorageActorHandle, String> {
        let sink: Arc<dyn PersistSink> = Arc::new(StorageSink(Arc::clone(&storage)));
        Self::spawn_with_sink(storage, sink).await
    }

    /// [`Self::spawn`] with session persists going to `sink`.
    pub(crate) async fn spawn_with_sink(
        storage: Arc<dyn post_cortex_storage::traits::GraphStorage>,
        sink: Arc<dyn PersistSink>,
    ) -> Result<StorageActorHandle, String> {
        let (sender, receiver) = channel(REQUEST_QUEUE_CAPACITY);
        let persister = SessionPersister::new(sink);

        let actor = Self {
            receiver,
            ctx: Arc::new(ActorContext {
                storage,
                persister: Arc::clone(&persister),
            }),
        };

        // Create confirmation channel for startup synchronization
        let (startup_tx, startup_rx) = oneshot::channel();

        // Spawn async actor task with startup confirmation
        tokio::spawn(async move {
            let _ = startup_tx.send(());
            actor.run_async().await;
        });

        startup_rx
            .await
            .map_err(|_| "Storage actor failed to start".to_string())?;

        Ok(StorageActorHandle { sender, persister })
    }

    async fn run_async(mut self) {
        info!("Storage actor started (async)");
        let limiter = Arc::new(Semaphore::new(MAX_CONCURRENT_REQUESTS));

        while let Some(message) = self.receiver.recv().await {
            if matches!(message, StorageMessage::Shutdown) {
                info!("Storage actor shutting down");
                break;
            }
            // Waiting for a permit stops intake, so a saturated backend
            // pushes back on senders through the bounded channel.
            let Ok(permit) = Arc::clone(&limiter).acquire_owned().await else {
                break;
            };
            let ctx = Arc::clone(&self.ctx);
            tokio::spawn(async move {
                ctx.handle_message_async(message).await;
                drop(permit);
            });
        }

        info!("Storage actor stopped");
    }
}

impl ActorContext {
    async fn handle_message_async(&self, message: StorageMessage) {
        trace!(
            "StorageActor: handling message {:?}",
            std::mem::discriminant(&message)
        );
        match message {
            StorageMessage::LoadSession {
                session_id,
                response_tx,
            } => {
                let result = match self.storage.load_session(session_id).await {
                    Ok(session) => Ok(Some(session)),
                    Err(e) if post_cortex_storage::is_not_found(&e) => Ok(None),
                    Err(e) => Err(format!("{e:#}")),
                };
                let _ = response_tx.send(result);
            }
            StorageMessage::SaveSession {
                session,
                response_tx,
            } => {
                let result = self
                    .persister
                    .exclusive(session.id(), self.storage.save_session(&session))
                    .await
                    .map_err(|e| e.to_string());
                let _ = response_tx.send(result);
            }
            StorageMessage::DeleteSession {
                session_id,
                live,
                response_tx,
            } => {
                let result = self
                    .persister
                    .delete(session_id, live.as_ref())
                    .await
                    .map(|_| true)
                    .map_err(|e| e.to_string());
                let _ = response_tx.send(result);
            }
            StorageMessage::ClearSessionEntities {
                session_id,
                response_tx,
            } => {
                let result = self
                    .persister
                    .exclusive(session_id, self.storage.clear_session_entities(session_id))
                    .await
                    .map_err(|e| e.to_string());
                let _ = response_tx.send(result);
            }
            StorageMessage::DeleteEntity {
                session_id,
                entity_name,
                response_tx,
            } => {
                let result = self
                    .persister
                    .exclusive(
                        session_id,
                        self.storage.delete_entity(session_id, &entity_name),
                    )
                    .await
                    .map_err(|e| e.to_string());
                let _ = response_tx.send(result);
            }
            StorageMessage::ListSessions { response_tx } => {
                let result = self
                    .storage
                    .list_sessions()
                    .await
                    .map_err(|e| e.to_string());
                let _ = response_tx.send(result);
            }
            StorageMessage::ListSessionSummaries { response_tx } => {
                let result = self
                    .storage
                    .list_session_summaries()
                    .await
                    .map_err(|e| e.to_string());
                let _ = response_tx.send(result);
            }
            StorageMessage::SaveCheckpoint {
                checkpoint,
                response_tx,
            } => {
                let result = self
                    .storage
                    .save_checkpoint(&checkpoint)
                    .await
                    .map_err(|e| e.to_string());
                let _ = response_tx.send(result);
            }
            StorageMessage::LoadCheckpoint {
                checkpoint_id,
                response_tx,
            } => {
                let result = self
                    .storage
                    .load_checkpoint(checkpoint_id)
                    .await
                    .map_err(|e| e.to_string());
                let _ = response_tx.send(result);
            }
            StorageMessage::SaveWorkspaceMetadata {
                workspace_id,
                name,
                description,
                session_ids,
                response_tx,
            } => {
                let result = self
                    .storage
                    .save_workspace_metadata(workspace_id, &name, &description, &session_ids)
                    .await
                    .map_err(|e| e.to_string());
                let _ = response_tx.send(result);
            }
            StorageMessage::DeleteWorkspace {
                workspace_id,
                response_tx,
            } => {
                let result = self
                    .storage
                    .delete_workspace(workspace_id)
                    .await
                    .map_err(|e| e.to_string());
                let _ = response_tx.send(result);
            }
            StorageMessage::AddSessionToWorkspace {
                workspace_id,
                session_id,
                role,
                response_tx,
            } => {
                let result = self
                    .storage
                    .add_session_to_workspace(workspace_id, session_id, role)
                    .await
                    .map_err(|e| e.to_string());
                let _ = response_tx.send(result);
            }
            StorageMessage::RemoveSessionFromWorkspace {
                workspace_id,
                session_id,
                response_tx,
            } => {
                let result = self
                    .storage
                    .remove_session_from_workspace(workspace_id, session_id)
                    .await
                    .map_err(|e| e.to_string());
                let _ = response_tx.send(result);
            }
            StorageMessage::ListAllWorkspaces { response_tx } => {
                let result = self
                    .storage
                    .list_workspaces()
                    .await
                    .map_err(|e| e.to_string());
                let _ = response_tx.send(result);
            }
            StorageMessage::BatchSaveUpdates {
                session_id,
                updates,
                response_tx,
            } => {
                let result = self
                    .persister
                    .exclusive(
                        session_id,
                        self.storage.batch_save_updates(session_id, updates),
                    )
                    .await
                    .map_err(|e| e.to_string());
                let _ = response_tx.send(result);
            }
            StorageMessage::RegisterSource {
                session_id,
                source_ref,
                response_tx,
            } => {
                let result = match timeout(
                    Duration::from_secs(5),
                    self.storage.register_source(session_id, source_ref),
                )
                .await
                {
                    Ok(inner) => inner.map_err(|e| e.to_string()),
                    Err(_) => Err("register_source timed out after 5s".to_string()),
                };
                let _ = response_tx.send(result);
            }
            StorageMessage::CheckFreshness {
                entry_id,
                file_hash,
                ast_hash,
                symbol_name,
                response_tx,
            } => {
                let result = self
                    .storage
                    .check_freshness_semantic(
                        &entry_id,
                        &file_hash,
                        ast_hash.as_deref(),
                        symbol_name.as_deref(),
                    )
                    .await
                    .map_err(|e| e.to_string());
                let _ = response_tx.send(result);
            }
            StorageMessage::CheckFreshnessBatch {
                entries,
                response_tx,
            } => {
                let result = self
                    .storage
                    .check_freshness_batch(entries)
                    .await
                    .map_err(|e| e.to_string());
                let _ = response_tx.send(result);
            }
            StorageMessage::InvalidateSource {
                file_path,
                response_tx,
            } => {
                let result = self
                    .storage
                    .invalidate_source(&file_path)
                    .await
                    .map_err(|e: anyhow::Error| e.to_string());
                let _ = response_tx.send(result);
            }
            StorageMessage::RegisterSymbolDependencies {
                from,
                to_symbols,
                response_tx,
            } => {
                let result = self
                    .storage
                    .register_symbol_dependencies(from, to_symbols)
                    .await
                    .map_err(|e| e.to_string());
                let _ = response_tx.send(result);
            }
            StorageMessage::CascadeInvalidate {
                changed,
                new_ast_hash,
                max_depth,
                response_tx,
            } => {
                let result = self
                    .storage
                    .cascade_invalidate(changed, new_ast_hash, max_depth)
                    .await
                    .map_err(|e| e.to_string());
                let _ = response_tx.send(result);
            }
            StorageMessage::GetStaleEntriesBySource {
                file_path,
                response_tx,
            } => {
                let result = self
                    .storage
                    .get_stale_entries_by_source(&file_path)
                    .await
                    .map_err(|e: anyhow::Error| e.to_string());
                let _ = response_tx.send(result);
            }
            StorageMessage::Shutdown => {} // Handled in main loop
        }
    }
}
