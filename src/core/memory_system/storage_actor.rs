use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::sync::{Mutex, oneshot};
use tokio::time::timeout;
use tracing::{debug, error, info, trace, warn};
use uuid::Uuid;

use crate::core::performance::PerformanceMonitor;
use post_cortex_proto::pb::{
    CascadeInvalidateReport, FreshnessEntry, SourceReference, SymbolId,
};
use crate::session::active_session::ActiveSession;

use super::config::OperationType;

/// Storage actor for handling all storage operations asynchronously
pub struct StorageActor {
    storage: Arc<dyn crate::storage::traits::GraphStorage>,
    receiver: UnboundedReceiver<StorageMessage>,
    performance_monitor: Arc<PerformanceMonitor>,
    /// Per-session locks for background persists. Ensures `PersistSessionAndUpdate`
    /// writes for the same session are serialized even though each runs in its
    /// own spawned task, preventing out-of-order session-blob writes.
    persist_locks: Arc<DashMap<Uuid, Arc<Mutex<()>>>>,
}

/// Handle for communicating with storage actor
#[derive(Clone)]
pub struct StorageActorHandle {
    sender: UnboundedSender<StorageMessage>,
}

/// Messages for storage actor
#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub enum StorageMessage {
    LoadSession {
        session_id: Uuid,
        response_tx: oneshot::Sender<Result<Option<ActiveSession>, String>>,
    },
    SaveSession {
        session: Box<ActiveSession>,
        response_tx: oneshot::Sender<Result<(), String>>,
    },
    DeleteSession {
        session_id: Uuid,
        response_tx: oneshot::Sender<Result<bool, String>>,
    },
    ClearSessionEntities {
        session_id: Uuid,
        response_tx: oneshot::Sender<Result<(), String>>,
    },
    DeleteEntity {
        session_id: Uuid,
        entity_name: String,
        response_tx: oneshot::Sender<Result<(), String>>,
    },
    ListSessions {
        response_tx: oneshot::Sender<Result<Vec<Uuid>, String>>,
    },
    SaveCheckpoint {
        checkpoint: crate::storage::rocksdb_storage::SessionCheckpoint,
        response_tx: oneshot::Sender<Result<(), String>>,
    },
    LoadCheckpoint {
        checkpoint_id: Uuid,
        response_tx: oneshot::Sender<Result<crate::storage::rocksdb_storage::SessionCheckpoint, String>>,
    },
    SaveWorkspaceMetadata {
        workspace_id: Uuid,
        name: String,
        description: String,
        session_ids: Vec<Uuid>,
        response_tx: oneshot::Sender<Result<(), String>>,
    },
    DeleteWorkspace {
        workspace_id: Uuid,
        response_tx: oneshot::Sender<Result<(), String>>,
    },
    AddSessionToWorkspace {
        workspace_id: Uuid,
        session_id: Uuid,
        role: crate::workspace::SessionRole,
        response_tx: oneshot::Sender<Result<(), String>>,
    },
    RemoveSessionFromWorkspace {
        workspace_id: Uuid,
        session_id: Uuid,
        response_tx: oneshot::Sender<Result<(), String>>,
    },
    ListAllWorkspaces {
        response_tx: oneshot::Sender<Result<Vec<crate::storage::rocksdb_storage::StoredWorkspace>, String>>,
    },
    BatchSaveUpdates {
        session_id: Uuid,
        updates: Vec<crate::core::context_update::ContextUpdate>,
        response_tx: oneshot::Sender<Result<(), String>>,
    },
    /// Fire-and-forget: persist session + updates without blocking caller.
    PersistSessionAndUpdate {
        session: Box<ActiveSession>,
        session_id: Uuid,
        updates: Vec<crate::core::context_update::ContextUpdate>,
    },
    RegisterSource {
        session_id: Uuid,
        source_ref: SourceReference,
        response_tx: oneshot::Sender<Result<(), String>>,
    },
    CheckFreshness {
        entry_id: String,
        file_hash: Vec<u8>,
        ast_hash: Option<Vec<u8>>,
        symbol_name: Option<String>,
        response_tx: oneshot::Sender<Result<FreshnessEntry, String>>,
    },
    CheckFreshnessBatch {
        entries: Vec<(String, Vec<u8>, Option<Vec<u8>>, Option<String>)>,
        response_tx: oneshot::Sender<Result<Vec<FreshnessEntry>, String>>,
    },
    InvalidateSource {
        file_path: String,
        response_tx: oneshot::Sender<Result<u32, String>>,
    },
    RegisterSymbolDependencies {
        from: SymbolId,
        to_symbols: Vec<SymbolId>,
        response_tx: oneshot::Sender<Result<u32, String>>,
    },
    CascadeInvalidate {
        changed: SymbolId,
        new_ast_hash: Vec<u8>,
        max_depth: u32,
        response_tx: oneshot::Sender<Result<CascadeInvalidateReport, String>>,
    },
    GetStaleEntriesBySource {
        file_path: String,
        response_tx: oneshot::Sender<Result<Vec<crate::storage::traits::StaleEntryInfo>, String>>,
    },
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
        self.sender
            .send(build_msg(response_tx))
            .map_err(|_| "Storage actor unavailable".to_string())?;

        let timeout_duration = op_type.timeout();
        match tokio::time::timeout(timeout_duration, response_rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err("Storage actor response channel closed".to_string()),
            Err(_) => Err(format!(
                "{} timed out after {}s",
                op_name(),
                timeout_duration.as_secs()
            )),
        }
    }

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

    /// Enqueue session + updates persistence without blocking.
    /// Returns immediately after sending the message. Errors are logged by the actor.
    pub fn persist_session_and_update_nowait(
        &self,
        session: ActiveSession,
        updates: Vec<crate::core::context_update::ContextUpdate>,
    ) {
        let session_id = session.id();
        if let Err(e) = self.sender.send(StorageMessage::PersistSessionAndUpdate {
            session: Box::new(session),
            session_id,
            updates,
        }) {
            warn!(
                "Failed to enqueue background persist for session {}: {}",
                session_id, e
            );
        }
    }

    pub async fn delete_session(&self, session_id: Uuid) -> Result<bool, String> {
        self.send_request(
            OperationType::Medium,
            || format!("DeleteSession {session_id}"),
            |response_tx| StorageMessage::DeleteSession {
                session_id,
                response_tx,
            },
        )
        .await
    }

    pub async fn delete_entity(
        &self,
        session_id: Uuid,
        entity_name: &str,
    ) -> Result<(), String> {
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

    pub async fn list_sessions(&self) -> Result<Vec<Uuid>, String> {
        self.send_request(
            OperationType::Slow,
            || "ListSessions".to_string(),
            |response_tx| StorageMessage::ListSessions { response_tx },
        )
        .await
    }

    pub async fn save_checkpoint(
        &self,
        checkpoint: &crate::storage::rocksdb_storage::SessionCheckpoint,
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

    pub async fn load_checkpoint(
        &self,
        checkpoint_id: Uuid,
    ) -> Result<crate::storage::rocksdb_storage::SessionCheckpoint, String> {
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

    pub async fn list_all_workspaces(
        &self,
    ) -> Result<Vec<crate::storage::rocksdb_storage::StoredWorkspace>, String> {
        self.send_request(
            OperationType::Slow,
            || "ListAllWorkspaces".to_string(),
            |response_tx| StorageMessage::ListAllWorkspaces { response_tx },
        )
        .await
    }

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

    pub async fn add_session_to_workspace(
        &self,
        workspace_id: Uuid,
        session_id: Uuid,
        role: crate::workspace::SessionRole,
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

    pub async fn batch_save_updates(
        &self,
        session_id: Uuid,
        updates: Vec<crate::core::context_update::ContextUpdate>,
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

    pub async fn check_freshness(
        &self,
        entry_id: String,
        file_hash: Vec<u8>,
    ) -> Result<FreshnessEntry, String> {
        self.check_freshness_semantic(entry_id, file_hash, None, None)
            .await
    }

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

    pub async fn get_stale_entries_by_source(
        &self,
        file_path: &str,
    ) -> Result<Vec<crate::storage::traits::StaleEntryInfo>, String> {
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
    pub async fn spawn(
        storage: Arc<dyn crate::storage::traits::GraphStorage>,
        performance_monitor: Arc<PerformanceMonitor>,
    ) -> Result<StorageActorHandle, String> {
        let (sender, receiver) = unbounded_channel();

        let actor = Self {
            storage,
            receiver,
            performance_monitor,
            persist_locks: Arc::new(DashMap::new()),
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

        Ok(StorageActorHandle { sender })
    }

    async fn run_async(mut self) {
        info!("Storage actor started (async)");

        while let Some(message) = self.receiver.recv().await {
            match message {
                StorageMessage::Shutdown => {
                    info!("Storage actor shutting down");
                    break;
                }
                msg => self.handle_message_async(msg).await,
            }
        }

        info!("Storage actor stopped");
    }

    async fn handle_message_async(&self, message: StorageMessage) {
        trace!(
            "StorageActor: handling message {:?}",
            std::mem::discriminant(&message)
        );
        let _timer = self.performance_monitor.start_timer("storage_operation");

        match message {
            StorageMessage::LoadSession {
                session_id,
                response_tx,
            } => {
                let result = match self.storage.load_session(session_id).await {
                    Ok(session) => Ok(Some(session)),
                    Err(_) => Ok(None), // Session not found is OK, not an error
                };
                let _ = response_tx.send(result);
            }
            StorageMessage::SaveSession {
                session,
                response_tx,
            } => {
                let result = self
                    .storage
                    .save_session(&session)
                    .await
                    .map_err(|e| e.to_string());
                let _ = response_tx.send(result);
            }
            StorageMessage::DeleteSession {
                session_id,
                response_tx,
            } => {
                let result = self
                    .storage
                    .delete_session(session_id)
                    .await
                    .map(|_| true)
                    .map_err(|e| e.to_string());
                // Drop the persist lock so it doesn't leak. Any in-flight persist
                // still holds its Arc<Mutex> clone so it'll finish cleanly.
                self.persist_locks.remove(&session_id);
                let _ = response_tx.send(result);
            }
            StorageMessage::ClearSessionEntities {
                session_id,
                response_tx,
            } => {
                let result = self
                    .storage
                    .clear_session_entities(session_id)
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
                    .storage
                    .delete_entity(session_id, &entity_name)
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
                    .storage
                    .batch_save_updates(session_id, updates)
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
            StorageMessage::PersistSessionAndUpdate {
                session,
                session_id,
                updates,
            } => {
                // Spawn as background task so we don't block the actor queue, BUT
                // serialize per-session via a tokio Mutex so concurrent persists
                // for the same session can't write the session blob out of order.
                let storage = Arc::clone(&self.storage);
                let lock = self
                    .persist_locks
                    .entry(session_id)
                    .or_insert_with(|| Arc::new(Mutex::new(())))
                    .clone();
                tokio::spawn(async move {
                    let _guard = lock.lock().await;
                    if let Err(e) = storage
                        .save_session_with_updates(&session, session_id, updates)
                        .await
                    {
                        warn!(
                            "Background persist failed for session {}: {}",
                            session_id, e
                        );
                    } else {
                        debug!("Background persist completed for session {}", session_id);
                    }
                });
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
