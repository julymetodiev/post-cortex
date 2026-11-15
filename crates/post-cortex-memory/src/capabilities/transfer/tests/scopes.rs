// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! Exports on every backend never report success with missing sessions: a
//! session that cannot be loaded fails the export (naming it), in every
//! scope; and `include_checkpoints` carries the checkpoints of the exported
//! sessions in every scope.

use crate::capabilities::transfer::tests::Backend;
use crate::capabilities::transfer::tests::{export, of};
use crate::capabilities::transfer::*;
use post_cortex_core::archive::ExportData;
use post_cortex_storage::RealRocksDBStorage;
use std::collections::HashMap;

use post_cortex_core::core::structured_context::StructuredContext;
use post_cortex_core::session::active_session::ActiveSession;
use post_cortex_core::workspace::SessionRole;
use uuid::Uuid;

use post_cortex_core::session::active_session::StoredSession;
use post_cortex_ports::storage::SessionCheckpoint;
use post_cortex_ports::storage::{SessionChanges, SessionStore, SessionSummary};
use std::sync::Arc;

/// Sessions of `inner`, with `bad` unreadable (a stored row that is
/// corrupt).
struct Corrupting {
    inner: Arc<dyn SessionStore>,
    bad: Uuid,
}

#[async_trait::async_trait]
impl SessionStore for Corrupting {
    async fn save_session(
        &self,
        session: &ActiveSession,
        changes: SessionChanges,
    ) -> post_cortex_ports::error::Result<()> {
        self.inner.save_session(session, changes).await
    }
    async fn load_session(&self, id: Uuid) -> post_cortex_ports::error::Result<StoredSession> {
        if id == self.bad {
            return Err(post_cortex_ports::error::corrupt(
                "session",
                id,
                "not a session",
            ));
        }
        self.inner.load_session(id).await
    }
    async fn delete_session(&self, id: Uuid) -> post_cortex_ports::error::Result<bool> {
        self.inner.delete_session(id).await
    }
    async fn list_sessions(&self) -> post_cortex_ports::error::Result<Vec<Uuid>> {
        self.inner.list_sessions().await
    }
    async fn list_session_summaries(
        &self,
    ) -> post_cortex_ports::error::Result<Vec<SessionSummary>> {
        self.inner.list_session_summaries().await
    }
    async fn session_exists(&self, id: Uuid) -> post_cortex_ports::error::Result<bool> {
        self.inner.session_exists(id).await
    }
    async fn replace_session(
        &self,
        session: &ActiveSession,
        vectorized: &[Uuid],
    ) -> post_cortex_ports::error::Result<()> {
        self.inner.replace_session(session, vectorized).await
    }
}

/// The archive ports of `storage` with session `bad` unreadable.
fn corrupting<B: Backend>(storage: &B, bad: Uuid) -> ArchivePorts {
    let mut ports = of(storage);
    ports.stores.sessions = Arc::new(Corrupting {
        inner: Arc::clone(&ports.stores.sessions),
        bad,
    });
    ports
}

fn checkpoint(session_id: Uuid) -> SessionCheckpoint {
    SessionCheckpoint {
        id: Uuid::new_v4(),
        session_id,
        created_at: chrono::Utc::now(),
        structured_context: StructuredContext::default(),
        recent_updates: Vec::new(),
        code_references: HashMap::new(),
        change_history: Vec::new(),
        total_updates: 0,
        context_quality_score: 1.0,
        compression_ratio: 1.0,
    }
}

fn with_checkpoints() -> ExportOptions {
    ExportOptions {
        include_checkpoints: true,
        ..Default::default()
    }
}

/// A workspace with one stored session, one member that does not load and
/// a session outside the workspace; each session but the missing one has a
/// checkpoint. Returns (workspace, member, missing, outsider, member's
/// checkpoint).
async fn fixture(storage: &impl Backend) -> (Uuid, Uuid, Uuid, Uuid, Uuid) {
    let (ws, member, missing, outsider) = (
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
    );
    storage
        .save_workspace_metadata(ws, "ws", &ws.to_string(), "d", &[])
        .await
        .unwrap();
    for sid in [member, outsider] {
        storage
            .save_session(
                &ActiveSession::new(sid, None, None, chrono::Utc::now()),
                SessionChanges::default(),
            )
            .await
            .unwrap();
    }
    for sid in [member, missing] {
        storage
            .add_session_to_workspace(ws, sid, SessionRole::Primary)
            .await
            .unwrap();
    }
    let kept = checkpoint(member);
    storage.save_checkpoint(&kept).await.unwrap();
    storage
        .save_checkpoint(&checkpoint(outsider))
        .await
        .unwrap();
    (ws, member, missing, outsider, kept.id)
}

fn assert_names(
    result: Result<ExportData, TransferError>,
    missing: Uuid,
    backend: &str,
    scope: &str,
) {
    let err = match result {
        Ok(export) => panic!(
            "{backend}: {scope} export reported success with {} sessions while {missing} did not load",
            export.sessions.len()
        ),
        Err(e) => format!("{e:#}"),
    };
    assert!(
        err.contains(&missing.to_string()),
        "{backend}: {scope}: {err}"
    );
}

fn checkpoint_ids(export: &ExportData) -> Vec<Uuid> {
    export.checkpoints.iter().map(|c| c.id).collect()
}

#[tokio::test]
async fn rocksdb_exports_fail_on_sessions_that_do_not_load() {
    let dir = tempfile::tempdir().unwrap();
    let storage = RealRocksDBStorage::new(dir.path()).await.unwrap();
    let (ws, member, missing, outsider, kept) = fixture(&storage).await;

    assert_names(
        export(
            &of(&storage),
            &ExportScope::Workspace(ws),
            &with_checkpoints(),
        )
        .await,
        missing,
        "rocksdb",
        "workspace",
    );
    let sessions = export(
        &of(&storage),
        &ExportScope::Sessions(vec![member]),
        &with_checkpoints(),
    )
    .await
    .unwrap();
    assert_eq!(checkpoint_ids(&sessions), [kept], "rocksdb: session scope");

    // A stored session whose row is corrupt fails the full export.
    assert_names(
        export(
            &corrupting(&storage, outsider),
            &ExportScope::All,
            &with_checkpoints(),
        )
        .await,
        outsider,
        "rocksdb",
        "full",
    );
}

#[tokio::test]
async fn surrealdb_exports_fail_on_sessions_that_do_not_load() {
    let storage = post_cortex_storage::SurrealDBStorage::new("mem://", None, None, None, None)
        .await
        .unwrap();
    let (ws, member, missing, outsider, kept) = fixture(&storage).await;

    assert_names(
        export(
            &of(&storage),
            &ExportScope::Workspace(ws),
            &with_checkpoints(),
        )
        .await,
        missing,
        "surrealdb",
        "workspace",
    );
    let sessions = export(
        &of(&storage),
        &ExportScope::Sessions(vec![member]),
        &with_checkpoints(),
    )
    .await
    .unwrap();
    assert_eq!(
        checkpoint_ids(&sessions),
        [kept],
        "surrealdb: session scope"
    );
    let full = export(&of(&storage), &ExportScope::All, &with_checkpoints())
        .await
        .unwrap();
    assert_eq!(full.checkpoints.len(), 2, "surrealdb: full scope");

    // A stored session whose row is corrupt fails the full export.
    assert_names(
        export(
            &corrupting(&storage, outsider),
            &ExportScope::All,
            &with_checkpoints(),
        )
        .await,
        outsider,
        "surrealdb",
        "full",
    );
}
