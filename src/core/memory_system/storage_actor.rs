use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::mpsc::{channel, unbounded_channel, Sender, UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;
use tokio::time::timeout;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::core::performance::PerformanceMonitor;
use crate::daemon::grpc_service::pb::{
    CascadeInvalidateReport, FreshnessEntry, SourceReference, SymbolId,
};
use crate::session::active_session::ActiveSession;

use super::config::OperationType;
use super::metrics::StorageStats;

/// Storage actor for handling all storage operations asynchronously
pub struct StorageActor {
    storage: Arc<dyn crate::storage::traits::GraphStorage>,
    receiver: UnboundedReceiver<StorageMessage>,
    performance_monitor: Arc<PerformanceMonitor>,
    operation_count: AtomicU64,
    load_count: AtomicU64,
    save_count: AtomicU64,
    delete_count: AtomicU64,
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
        response_tx: Sender<Result<Option<ActiveSession>, String>>,
    },
    SaveSession {
        session: Box<ActiveSession>,
        response_tx: Sender<Result<(), String>>,
    },
    DeleteSession {
        session_id: Uuid,
        response_tx: Sender<Result<bool, String>>,
    },
    ClearSessionEntities {
        session_id: Uuid,
        response_tx: Sender<Result<(), String>>,
    },
    DeleteEntity {
        session_id: Uuid,
        entity_name: String,
        response_tx: Sender<Result<(), String>>,
    },
    ListSessions {
        response_tx: Sender<Result<Vec<Uuid>, String>>,
    },
    GetStats {
        response_tx: Sender<StorageStats>,
    },
    SaveCheckpoint {
        checkpoint: crate::storage::rocksdb_storage::SessionCheckpoint,
        response_tx: Sender<Result<(), String>>,
    },
    LoadCheckpoint {
        checkpoint_id: Uuid,
        response_tx: Sender<Result<crate::storage::rocksdb_storage::SessionCheckpoint, String>>,
    },
    SaveWorkspaceMetadata {
        workspace_id: Uuid,
        name: String,
        description: String,
        session_ids: Vec<Uuid>,
        response_tx: Sender<Result<(), String>>,
    },
    DeleteWorkspace {
        workspace_id: Uuid,
        response_tx: Sender<Result<(), String>>,
    },
    AddSessionToWorkspace {
        workspace_id: Uuid,
        session_id: Uuid,
        role: crate::workspace::SessionRole,
        response_tx: Sender<Result<(), String>>,
    },
    RemoveSessionFromWorkspace {
        workspace_id: Uuid,
        session_id: Uuid,
        response_tx: Sender<Result<(), String>>,
    },
    ListAllWorkspaces {
        response_tx: Sender<Result<Vec<crate::storage::rocksdb_storage::StoredWorkspace>, String>>,
    },
    BatchSaveUpdates {
        session_id: Uuid,
        updates: Vec<crate::core::context_update::ContextUpdate>,
        response_tx: Sender<Result<(), String>>,
    },
    /// Fire-and-forget: persist session + updates without blocking caller.
    PersistSessionAndUpdate {
        session: Box<ActiveSession>,
        session_id: Uuid,
        updates: Vec<crate::core::context_update::ContextUpdate>,
    },
    FindRelatedEntities {
        session_id: Uuid,
        entity_name: String,
        response_tx: Sender<Result<Vec<String>, String>>,
    },
    FindShortestPath {
        session_id: Uuid,
        from_entity: String,
        to_entity: String,
        response_tx: Sender<Result<Option<Vec<String>>, String>>,
    },
    RegisterSource {
        session_id: Uuid,
        source_ref: SourceReference,
        response_tx: Sender<Result<(), String>>,
    },
    CheckFreshness {
        entry_id: String,
        file_hash: Vec<u8>,
        ast_hash: Option<Vec<u8>>,
        symbol_name: Option<String>,
        response_tx: Sender<Result<FreshnessEntry, String>>,
    },
    CheckFreshnessBatch {
        entries: Vec<(String, Vec<u8>, Option<Vec<u8>>, Option<String>)>,
        response_tx: Sender<Result<Vec<FreshnessEntry>, String>>,
    },
    InvalidateSource {
        file_path: String,
        response_tx: Sender<Result<u32, String>>,
    },
    RegisterSymbolDependencies {
        from: SymbolId,
        to_symbols: Vec<SymbolId>,
        response_tx: Sender<Result<u32, String>>,
    },
    CascadeInvalidate {
        changed: SymbolId,
        new_ast_hash: Vec<u8>,
        max_depth: u32,
        response_tx: Sender<Result<CascadeInvalidateReport, String>>,
    },
    GetStaleEntriesBySource {
        file_path: String,
        response_tx: Sender<Result<Vec<crate::storage::traits::StaleEntryInfo>, String>>,
    },
    Shutdown,
}


impl StorageActorHandle {
    /// Execute an operation with the specified timeout type
    async fn execute_with_timeout<T, F>(
        &self,
        op_type: OperationType,
        op_name: &str,
        future: F,
    ) -> Result<T, String>
    where
        F: std::future::Future<Output = Option<Result<T, String>>>,
    {
        let timeout = op_type.timeout();
        debug!(
            "StorageHandle: {} with {:?} timeout ({}s)",
            op_name,
            op_type,
            timeout.as_secs()
        );

        tokio::time::timeout(timeout, future)
            .await
            .map_err(|_| format!("{} timed out after {}s", op_name, timeout.as_secs()))?
            .ok_or_else(|| "Storage actor response channel closed".to_string())?
    }

    pub async fn load_session(&self, session_id: Uuid) -> Result<Option<ActiveSession>, String> {
        let (response_tx, mut response_rx) = channel::<Result<Option<ActiveSession>, String>>(1);

        self.sender
            .send(StorageMessage::LoadSession {
                session_id,
                response_tx,
            })
            .map_err(|_| "Storage actor unavailable".to_string())?;

        self.execute_with_timeout(
            OperationType::Fast,
            &format!("LoadSession {}", session_id),
            response_rx.recv(),
        )
        .await
    }

    pub async fn save_session(&self, session: ActiveSession) -> Result<(), String> {
        let (response_tx, mut response_rx) = channel::<Result<(), String>>(1);
        let session_id = session.id();

        self.sender
            .send(StorageMessage::SaveSession {
                session: Box::new(session),
                response_tx,
            })
            .map_err(|_| "Storage actor unavailable".to_string())?;

        self.execute_with_timeout(
            OperationType::Medium,
            &format!("SaveSession {}", session_id),
            response_rx.recv(),
        )
        .await
    }

    /// Clear all entities and relationships for a session from storage.
    async fn clear_session_entities(&self, session_id: Uuid) -> Result<(), String> {
        let (response_tx, mut response_rx) = channel::<Result<(), String>>(1);

        self.sender
            .send(StorageMessage::ClearSessionEntities {
                session_id,
                response_tx,
            })
            .map_err(|_| "Storage actor unavailable".to_string())?;

        self.execute_with_timeout(
            OperationType::Medium,
            &format!("ClearSessionEntities {}", session_id),
            response_rx.recv(),
        )
        .await
    }

    /// Rebuild entity graph for a session by clearing it and replaying all stored updates.
    /// Returns (entities_before, entities_after) counts.
    pub async fn rebuild_entity_graph(
        &self,
        session_id: Uuid,
    ) -> Result<(usize, usize), String> {
        // Load session
        let session = self
            .load_session(session_id)
            .await?
            .ok_or_else(|| format!("Session {} not found", session_id))?;

        let mut session = session;
        let stats = session
            .rebuild_entity_graph_from_updates()
            .await
            .map_err(|e| format!("Rebuild failed: {}", e))?;

        // Clear old entities/relationships from storage before saving rebuilt graph
        self.clear_session_entities(session_id).await?;

        // Save rebuilt session (with clean entity graph)
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
        let (response_tx, mut response_rx) = channel::<Result<bool, String>>(1);

        self.sender
            .send(StorageMessage::DeleteSession {
                session_id,
                response_tx,
            })
            .map_err(|_| "Storage actor unavailable".to_string())?;

        self.execute_with_timeout(
            OperationType::Medium,
            &format!("DeleteSession {}", session_id),
            response_rx.recv(),
        )
        .await
    }

    pub async fn delete_entity(
        &self,
        session_id: Uuid,
        entity_name: &str,
    ) -> Result<(), String> {
        let (response_tx, mut response_rx) = channel::<Result<(), String>>(1);

        self.sender
            .send(StorageMessage::DeleteEntity {
                session_id,
                entity_name: entity_name.to_string(),
                response_tx,
            })
            .map_err(|_| "Storage actor unavailable".to_string())?;

        self.execute_with_timeout(
            OperationType::Medium,
            &format!("DeleteEntity {}/{}", session_id, entity_name),
            response_rx.recv(),
        )
        .await
    }

    pub async fn list_sessions(&self) -> Result<Vec<Uuid>, String> {
        let (response_tx, mut response_rx) = channel::<Result<Vec<Uuid>, String>>(1);

        self.sender
            .send(StorageMessage::ListSessions { response_tx })
            .map_err(|_| "Storage actor unavailable".to_string())?;

        self.execute_with_timeout(OperationType::Slow, "ListSessions", response_rx.recv())
            .await
    }

    pub async fn save_checkpoint(
        &self,
        checkpoint: &crate::storage::rocksdb_storage::SessionCheckpoint,
    ) -> Result<(), String> {
        let (response_tx, mut response_rx) = channel::<Result<(), String>>(1);

        self.sender
            .send(StorageMessage::SaveCheckpoint {
                checkpoint: checkpoint.clone(),
                response_tx,
            })
            .map_err(|_| "Storage actor unavailable".to_string())?;

        self.execute_with_timeout(OperationType::Medium, "SaveCheckpoint", response_rx.recv())
            .await
    }

    pub async fn load_checkpoint(
        &self,
        checkpoint_id: uuid::Uuid,
    ) -> Result<crate::storage::rocksdb_storage::SessionCheckpoint, String> {
        let (response_tx, mut response_rx) =
            channel::<Result<crate::storage::rocksdb_storage::SessionCheckpoint, String>>(1);

        self.sender
            .send(StorageMessage::LoadCheckpoint {
                checkpoint_id,
                response_tx,
            })
            .map_err(|_| "Storage actor unavailable".to_string())?;

        self.execute_with_timeout(
            OperationType::Fast,
            &format!("LoadCheckpoint {}", checkpoint_id),
            response_rx.recv(),
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
        let (response_tx, mut response_rx) = channel::<Result<(), String>>(1);

        self.sender
            .send(StorageMessage::SaveWorkspaceMetadata {
                workspace_id,
                name: name.to_string(),
                description: description.to_string(),
                session_ids: session_ids.to_vec(),
                response_tx,
            })
            .map_err(|_| "Storage actor unavailable".to_string())?;

        self.execute_with_timeout(
            OperationType::Medium,
            "SaveWorkspaceMetadata",
            response_rx.recv(),
        )
        .await
    }

    pub async fn list_all_workspaces(
        &self,
    ) -> Result<Vec<crate::storage::rocksdb_storage::StoredWorkspace>, String> {
        let (response_tx, mut response_rx) =
            channel::<Result<Vec<crate::storage::rocksdb_storage::StoredWorkspace>, String>>(1);

        self.sender
            .send(StorageMessage::ListAllWorkspaces { response_tx })
            .map_err(|_| "Storage actor unavailable".to_string())?;

        self.execute_with_timeout(OperationType::Slow, "ListAllWorkspaces", response_rx.recv())
            .await
    }

    pub async fn delete_workspace(&self, workspace_id: Uuid) -> Result<(), String> {
        let (response_tx, mut response_rx) = channel::<Result<(), String>>(1);

        self.sender
            .send(StorageMessage::DeleteWorkspace {
                workspace_id,
                response_tx,
            })
            .map_err(|_| "Storage actor unavailable".to_string())?;

        self.execute_with_timeout(
            OperationType::Medium,
            &format!("DeleteWorkspace {}", workspace_id),
            response_rx.recv(),
        )
        .await
    }

    pub async fn add_session_to_workspace(
        &self,
        workspace_id: Uuid,
        session_id: Uuid,
        role: crate::workspace::SessionRole,
    ) -> Result<(), String> {
        let (response_tx, mut response_rx) = channel::<Result<(), String>>(1);

        self.sender
            .send(StorageMessage::AddSessionToWorkspace {
                workspace_id,
                session_id,
                role,
                response_tx,
            })
            .map_err(|_| "Storage actor unavailable".to_string())?;

        self.execute_with_timeout(
            OperationType::Fast,
            "AddSessionToWorkspace",
            response_rx.recv(),
        )
        .await
    }

    pub async fn remove_session_from_workspace(
        &self,
        workspace_id: Uuid,
        session_id: Uuid,
    ) -> Result<(), String> {
        let (response_tx, mut response_rx) = channel::<Result<(), String>>(1);

        self.sender
            .send(StorageMessage::RemoveSessionFromWorkspace {
                workspace_id,
                session_id,
                response_tx,
            })
            .map_err(|_| "Storage actor unavailable".to_string())?;

        self.execute_with_timeout(
            OperationType::Fast,
            "RemoveSessionFromWorkspace",
            response_rx.recv(),
        )
        .await
    }

    pub async fn batch_save_updates(
        &self,
        session_id: Uuid,
        updates: Vec<crate::core::context_update::ContextUpdate>,
    ) -> Result<(), String> {
        let (response_tx, mut response_rx) = channel::<Result<(), String>>(1);

        self.sender
            .send(StorageMessage::BatchSaveUpdates {
                session_id,
                updates,
                response_tx,
            })
            .map_err(|_| "Storage actor unavailable".to_string())?;

        self.execute_with_timeout(
            OperationType::Medium,
            &format!("BatchSaveUpdates {}", session_id),
            response_rx.recv(),
        )
        .await
    }

    pub async fn register_source(
        &self,
        session_id: Uuid,
        source_ref: SourceReference,
    ) -> Result<(), String> {
        let (response_tx, mut response_rx) = channel(1);

        self.sender
            .send(StorageMessage::RegisterSource {
                session_id,
                source_ref,
                response_tx,
            })
            .map_err(|_| "Storage actor unavailable".to_string())?;

        self.execute_with_timeout(
            OperationType::Medium,
            "RegisterSource",
            response_rx.recv(),
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
        let (response_tx, mut response_rx) = channel(1);

        self.sender
            .send(StorageMessage::CheckFreshness {
                entry_id: entry_id.clone(),
                file_hash,
                ast_hash,
                symbol_name,
                response_tx,
            })
            .map_err(|_| "Storage actor unavailable".to_string())?;

        self.execute_with_timeout(
            OperationType::Fast,
            &format!("CheckFreshness {}", entry_id),
            response_rx.recv(),
        )
        .await
    }

    pub async fn check_freshness_batch(
        &self,
        entries: Vec<(String, Vec<u8>, Option<Vec<u8>>, Option<String>)>,
    ) -> Result<Vec<FreshnessEntry>, String> {
        let (response_tx, mut response_rx) = channel(1);

        self.sender
            .send(StorageMessage::CheckFreshnessBatch {
                entries,
                response_tx,
            })
            .map_err(|_| "Storage actor unavailable".to_string())?;

        self.execute_with_timeout(
            OperationType::Medium,
            "CheckFreshnessBatch",
            response_rx.recv(),
        )
        .await
    }

    pub async fn invalidate_source(
        &self,
        file_path: &str,
    ) -> Result<u32, String> {
        let (response_tx, mut response_rx) = channel(1);

        self.sender
            .send(StorageMessage::InvalidateSource {
                file_path: file_path.to_string(),
                response_tx,
            })
            .map_err(|_| "Storage actor unavailable".to_string())?;

        self.execute_with_timeout(
            OperationType::Medium,
            "InvalidateSource",
            response_rx.recv(),
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
        let (response_tx, mut response_rx) = channel(1);

        self.sender
            .send(StorageMessage::RegisterSymbolDependencies {
                from,
                to_symbols,
                response_tx,
            })
            .map_err(|_| "Storage actor unavailable".to_string())?;

        self.execute_with_timeout(
            OperationType::Medium,
            "RegisterSymbolDependencies",
            response_rx.recv(),
        )
        .await
    }

    pub async fn cascade_invalidate(
        &self,
        changed: SymbolId,
        new_ast_hash: Vec<u8>,
        max_depth: u32,
    ) -> Result<CascadeInvalidateReport, String> {
        let (response_tx, mut response_rx) = channel(1);

        self.sender
            .send(StorageMessage::CascadeInvalidate {
                changed,
                new_ast_hash,
                max_depth,
                response_tx,
            })
            .map_err(|_| "Storage actor unavailable".to_string())?;

        self.execute_with_timeout(
            OperationType::Medium,
            "CascadeInvalidate",
            response_rx.recv(),
        )
        .await
    }

    pub async fn get_stale_entries_by_source(
        &self,
        file_path: &str,
    ) -> Result<Vec<crate::storage::traits::StaleEntryInfo>, String> {
        let (response_tx, mut response_rx) = channel(1);

        self.sender
            .send(StorageMessage::GetStaleEntriesBySource {
                file_path: file_path.to_string(),
                response_tx,
            })
            .map_err(|_| "Storage actor unavailable".to_string())?;

        self.execute_with_timeout(
            OperationType::Medium,
            "GetStaleEntriesBySource",
            response_rx.recv(),
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
            operation_count: AtomicU64::new(0),
            load_count: AtomicU64::new(0),
            save_count: AtomicU64::new(0),
            delete_count: AtomicU64::new(0),
        };

        // Create confirmation channel for startup synchronization
        let (startup_tx, startup_rx) = oneshot::channel();

        // Spawn async actor task with startup confirmation
        tokio::spawn(async move {
            // Send confirmation that actor is ready
            let _ = startup_tx.send(());
            actor.run_async().await;
        });

        // Wait for actor to be ready before returning handle
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
        tracing::info!(
            "StorageActor: Handling message: {:?}",
            std::mem::discriminant(&message)
        );
        let _timer = self.performance_monitor.start_timer("storage_operation");
        self.operation_count.fetch_add(1, Ordering::Relaxed);

        match message {
            StorageMessage::LoadSession {
                session_id,
                response_tx,
            } => {
                self.load_count.fetch_add(1, Ordering::Relaxed);
                debug!("StorageActor: Loading session {}", session_id);
                let result = match self.storage.load_session(session_id).await {
                    Ok(session) => Ok(Some(session)),
                    Err(_) => Ok(None), // Session not found is OK, not an error
                };
                let _ = response_tx.send(result).await;
            }
            StorageMessage::SaveSession {
                session,
                response_tx,
            } => {
                self.save_count.fetch_add(1, Ordering::Relaxed);
                let result = self
                    .storage
                    .save_session(&session)
                    .await
                    .map_err(|e| e.to_string());
                let _ = response_tx.send(result).await;
            }
            StorageMessage::DeleteSession {
                session_id,
                response_tx,
            } => {
                self.delete_count.fetch_add(1, Ordering::Relaxed);
                let result = self
                    .storage
                    .delete_session(session_id)
                    .await
                    .map(|_| true)
                    .map_err(|e| e.to_string());
                let _ = response_tx.send(result).await;
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
                let _ = response_tx.send(result).await;
            }
            StorageMessage::DeleteEntity {
                session_id,
                entity_name,
                response_tx,
            } => {
                self.delete_count.fetch_add(1, Ordering::Relaxed);
                let result = self
                    .storage
                    .delete_entity(session_id, &entity_name)
                    .await
                    .map_err(|e| e.to_string());
                let _ = response_tx.send(result).await;
            }
            StorageMessage::ListSessions { response_tx } => {
                self.load_count.fetch_add(1, Ordering::Relaxed);
                let result = self
                    .storage
                    .list_sessions()
                    .await
                    .map_err(|e| e.to_string());
                let _ = response_tx.send(result).await;
            }
            StorageMessage::GetStats { response_tx } => {
                let stats = StorageStats {
                    total_operations: self.operation_count.load(Ordering::Relaxed),
                    load_operations: self.load_count.load(Ordering::Relaxed),
                    save_operations: self.save_count.load(Ordering::Relaxed),
                    delete_operations: self.delete_count.load(Ordering::Relaxed),
                    avg_operation_time_ns: 0,
                    last_operation_timestamp: SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .expect("System time before UNIX epoch")
                        .as_secs(),
                };
                let _ = response_tx.send(stats).await;
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
                let _ = response_tx.send(result).await;
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
                let _ = response_tx.send(result).await;
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
                let _ = response_tx.send(result).await;
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
                let _ = response_tx.send(result).await;
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
                let _ = response_tx.send(result).await;
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
                let _ = response_tx.send(result).await;
            }
            StorageMessage::ListAllWorkspaces { response_tx } => {
                let result = self
                    .storage
                    .list_workspaces()
                    .await
                    .map_err(|e| e.to_string());
                let _ = response_tx.send(result).await;
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
                let _ = response_tx.send(result).await;
            }
            StorageMessage::FindRelatedEntities {
                session_id,
                entity_name,
                response_tx,
            } => {
                // Load session and access its entity graph
                let result = match self.storage.load_session(session_id).await {
                    Ok(session) => {
                        let related = session.entity_graph.find_related_entities(&entity_name);
                        Ok(related)
                    }
                    Err(e) => Err(e.to_string()),
                };
                let _ = response_tx.send(result).await;
            }
            StorageMessage::FindShortestPath {
                session_id,
                from_entity,
                to_entity,
                response_tx,
            } => {
                // Load session and access its entity graph
                let result = match self.storage.load_session(session_id).await {
                    Ok(session) => {
                        let path = session.entity_graph.find_shortest_path(&from_entity, &to_entity);
                        Ok(path)
                    }
                    Err(e) => Err(e.to_string()),
                };
                let _ = response_tx.send(result).await;
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
                    Err(_) => Err(
                        "register_source timed out after 5s".to_string()
                    ),
                };
                let _ = response_tx.send(result).await;
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
                let _ = response_tx.send(result).await;
            }
            StorageMessage::CheckFreshnessBatch { entries, response_tx } => {
                let result = self
                    .storage
                    .check_freshness_batch(entries)
                    .await
                    .map_err(|e| e.to_string());
                let _ = response_tx.send(result).await;
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
                let _ = response_tx.send(result).await;
            }
            StorageMessage::PersistSessionAndUpdate {
                session,
                session_id,
                updates,
            } => {
                self.save_count.fetch_add(1, Ordering::Relaxed);
                // Spawn as background task so we don't block the actor queue.
                // PersistSessionAndUpdate is fire-and-forget (no response_tx),
                // so it's safe to run outside the actor loop.
                let storage = Arc::clone(&self.storage);
                tokio::spawn(async move {
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
                let _ = response_tx.send(result).await;
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
                let _ = response_tx.send(result).await;
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
                let _ = response_tx.send(result).await;
            }
            StorageMessage::Shutdown => {} // Handled in main loop
        }
    }
}
