// Copyright (c) 2025, 2026 Julius ML
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.

//! Workspace rows (`workspace:{id}`) and memberships
//! (`ws_session:{workspace}:{session}`), layouts in [`crate::rocksdb_storage::rows`].
//!
//! Every mutation of a workspace, its delete included, runs under the
//! workspace's lock as one `WriteBatch`: a read-modify-write never
//! resurrects a workspace deleted meanwhile, and a membership is only
//! written while its workspace exists.

use crate::port_error::PortResult;
use crate::port_error::Result;
use async_trait::async_trait;
use rocksdb::{DB, WriteBatch};
use std::collections::{HashMap, HashSet};
use tracing::debug;
use uuid::Uuid;

use post_cortex_core::core::card::CardScope;
use post_cortex_core::workspace::{SessionRole, WorkspaceProject};

use crate::rocksdb_storage::RealRocksDBStorage;
use crate::rocksdb_storage::core::{blocking, keys_with_prefix, scan_prefix};
use crate::rocksdb_storage::rows::{
    MembershipRow, WorkspaceRow, decode_membership, decode_workspace, encode_membership,
    encode_workspace,
};
use post_cortex_ports::error::workspace_not_found;
use post_cortex_ports::storage::StoredWorkspace;
use post_cortex_ports::storage::WorkspaceStore;
use post_cortex_ports::storage::WorkspaceUpdate;

fn workspace_key(workspace_id: Uuid) -> String {
    format!("workspace:{workspace_id}")
}

fn membership_prefix(workspace_id: Uuid) -> String {
    format!("ws_session:{workspace_id}:")
}

fn membership_key(workspace_id: Uuid, session_id: Uuid) -> String {
    format!("ws_session:{workspace_id}:{session_id}")
}

/// Unix seconds now.
fn now_secs() -> u64 {
    chrono::Utc::now().timestamp().max(0) as u64
}

fn load_row(db: &DB, workspace_id: Uuid) -> Result<Option<WorkspaceRow>> {
    let key = workspace_key(workspace_id);
    db.get(key.as_bytes())?
        .map(|row| decode_workspace(&key, &row))
        .transpose()
}

/// The stored row, or a not-found error.
fn existing_row(db: &DB, workspace_id: Uuid) -> Result<WorkspaceRow> {
    Ok(load_row(db, workspace_id)?.ok_or_else(|| workspace_not_found(workspace_id))?)
}

/// The workspace's memberships by session.
fn memberships(db: &DB, workspace_id: Uuid) -> Result<HashMap<Uuid, MembershipRow>> {
    let mut members = HashMap::new();
    for item in scan_prefix(db, membership_prefix(workspace_id).as_bytes()) {
        let (key, row) = item?;
        let membership = decode_membership(&String::from_utf8_lossy(&key), &row)?;
        members.insert(membership.session_id, membership);
    }
    Ok(members)
}

fn put_row(batch: &mut WriteBatch, row: &WorkspaceRow) -> Result<()> {
    batch.put(workspace_key(row.id).as_bytes(), encode_workspace(row)?);
    Ok(())
}

fn put_membership(batch: &mut WriteBatch, membership: &MembershipRow) -> Result<()> {
    batch.put(
        membership_key(membership.workspace_id, membership.session_id).as_bytes(),
        encode_membership(membership)?,
    );
    Ok(())
}

impl RealRocksDBStorage {
    /// Run `f` on the database under `workspace_id`'s lock, off the runtime.
    async fn with_workspace<T, F>(&self, workspace_id: Uuid, f: F) -> Result<T>
    where
        F: FnOnce(&DB) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let db = self.db.clone();
        let locks = self.session_locks.clone();
        blocking(move || {
            let _guard = locks.lock(workspace_id);
            f(&db)
        })
        .await
    }
}

#[async_trait]
impl WorkspaceStore for RealRocksDBStorage {
    /// Create the workspace (with `slug`, the remote reranker forbidden) or
    /// rename it. Its slug, creation time, projects, archive state and
    /// remote reranking stay; each of `session_ids` that is not a member yet
    /// becomes one as `Primary` (members keep their role).
    async fn save_workspace_metadata(
        &self,
        workspace_id: Uuid,
        name: &str,
        slug: &str,
        description: &str,
        session_ids: &[Uuid],
    ) -> PortResult<()> {
        self.breaker
            .guard(async {
                let (name, description) = (name.to_string(), description.to_string());
                let slug = slug.to_string();
                let session_ids = session_ids.to_vec();
                self.with_workspace(workspace_id, move |db| {
                    let now = now_secs();
                    let row = match load_row(db, workspace_id)? {
                        Some(row) => WorkspaceRow {
                            name,
                            description,
                            ..row
                        },
                        None => WorkspaceRow {
                            id: workspace_id,
                            name,
                            slug,
                            description,
                            created_at: now,
                            projects: Vec::new(),
                            archived_at: None,
                            remote_rerank: false,
                        },
                    };
                    let members = memberships(db, workspace_id)?;
                    let mut batch = WriteBatch::default();
                    put_row(&mut batch, &row)?;
                    for session_id in session_ids.iter().filter(|id| !members.contains_key(id)) {
                        let membership = MembershipRow::new(
                            workspace_id,
                            *session_id,
                            SessionRole::Primary,
                            now,
                        );
                        put_membership(&mut batch, &membership)?;
                    }
                    db.write(batch)?;
                    debug!("RealRocksDBStorage: Workspace {workspace_id} saved");
                    Ok(())
                })
                .await
            })
            .await
    }

    /// Replace the projects of a stored workspace.
    async fn save_workspace_projects(
        &self,
        workspace_id: Uuid,
        projects: &[WorkspaceProject],
    ) -> PortResult<()> {
        self.breaker
            .guard(async {
                let projects = WorkspaceRow::from_projects(projects);
                self.with_workspace(workspace_id, move |db| {
                    let row = WorkspaceRow {
                        projects,
                        ..existing_row(db, workspace_id)?
                    };
                    let mut batch = WriteBatch::default();
                    put_row(&mut batch, &row)?;
                    db.write(batch)?;
                    Ok(())
                })
                .await
            })
            .await
    }

    /// Change the name, description, archive state or remote reranking of a
    /// stored workspace.
    async fn update_workspace(
        &self,
        workspace_id: Uuid,
        update: &WorkspaceUpdate,
    ) -> PortResult<()> {
        self.breaker
            .guard(async {
                let update = update.clone();
                self.with_workspace(workspace_id, move |db| {
                    let mut row = existing_row(db, workspace_id)?;
                    if let Some(name) = update.name {
                        row.name = name;
                    }
                    if let Some(description) = update.description {
                        row.description = description;
                    }
                    if let Some(archived_at) = update.archived_at {
                        row.archived_at = archived_at;
                    }
                    if let Some(allowed) = update.remote_rerank {
                        row.remote_rerank = allowed;
                    }
                    let mut batch = WriteBatch::default();
                    put_row(&mut batch, &row)?;
                    db.write(batch)?;
                    Ok(())
                })
                .await
            })
            .await
    }

    /// Write `workspace` as it is (creation time, projects, archive state,
    /// remote reranking and members with their roles) in place of whatever is stored under
    /// its id, in one batch. Its card and radar dismissals stay.
    async fn restore_workspace(&self, workspace: &StoredWorkspace) -> PortResult<()> {
        self.breaker
            .guard(async {
                let workspace = workspace.clone();
                self.with_workspace(workspace.id, move |db| {
                    let keep: HashSet<Uuid> =
                        workspace.sessions.iter().map(|(id, _)| *id).collect();
                    let members = memberships(db, workspace.id)?;
                    let now = now_secs();
                    let mut batch = WriteBatch::default();
                    put_row(
                        &mut batch,
                        &WorkspaceRow {
                            id: workspace.id,
                            name: workspace.name,
                            slug: workspace.slug,
                            description: workspace.description,
                            created_at: workspace.created_at,
                            projects: WorkspaceRow::from_projects(&workspace.projects),
                            archived_at: workspace.archived_at,
                            remote_rerank: workspace.remote_rerank,
                        },
                    )?;
                    for session_id in members.keys().filter(|id| !keep.contains(id)) {
                        batch.delete(membership_key(workspace.id, *session_id).as_bytes());
                    }
                    for (session_id, role) in &workspace.sessions {
                        let added_at = members.get(session_id).map_or(now, |m| m.added_at);
                        let membership =
                            MembershipRow::new(workspace.id, *session_id, *role, added_at);
                        put_membership(&mut batch, &membership)?;
                    }
                    db.write(batch)?;
                    Ok(())
                })
                .await
            })
            .await
    }

    /// Delete the workspace, its memberships, its card, its radar
    /// dismissals and its documents in one batch.
    async fn delete_workspace(&self, workspace_id: Uuid) -> PortResult<()> {
        self.breaker
            .guard(async {
                self.with_workspace(workspace_id, move |db| {
                    let mut batch = WriteBatch::default();
                    batch.delete(workspace_key(workspace_id).as_bytes());
                    for key in keys_with_prefix(db, membership_prefix(workspace_id).as_bytes())? {
                        batch.delete(&key);
                    }
                    crate::rocksdb_storage::cards::delete_card_in(
                        db,
                        &mut batch,
                        CardScope::Workspace(workspace_id),
                    )?;
                    crate::rocksdb_storage::dismissals::delete_dismissals_in(
                        db,
                        &mut batch,
                        workspace_id,
                    )?;
                    crate::rocksdb_storage::documents::delete_documents_in(
                        db,
                        &mut batch,
                        workspace_id,
                    )?;
                    db.write(batch)?;
                    debug!("RealRocksDBStorage: Workspace {workspace_id} deleted");
                    Ok(())
                })
                .await
            })
            .await
    }

    /// Every workspace with its members.
    async fn list_workspaces(&self) -> PortResult<Vec<StoredWorkspace>> {
        self.breaker
            .guard(async {
                let db = self.db.clone();
                blocking(move || {
                    let mut members: HashMap<Uuid, Vec<(Uuid, SessionRole)>> = HashMap::new();
                    for item in scan_prefix(&db, b"ws_session:") {
                        let (key, row) = item?;
                        let membership = decode_membership(&String::from_utf8_lossy(&key), &row)?;
                        members
                            .entry(membership.workspace_id)
                            .or_default()
                            .push((membership.session_id, membership.role()?));
                    }
                    let mut workspaces = Vec::new();
                    for item in scan_prefix(&db, b"workspace:") {
                        let (key, row) = item?;
                        let row = decode_workspace(&String::from_utf8_lossy(&key), &row)?;
                        let sessions = members.remove(&row.id).unwrap_or_default();
                        workspaces.push(row.into_stored(sessions));
                    }
                    Ok(workspaces)
                })
                .await
            })
            .await
    }

    /// Make `session_id` a member of the workspace with `role`; fails with a
    /// not-found error if there is no such workspace.
    async fn add_session_to_workspace(
        &self,
        workspace_id: Uuid,
        session_id: Uuid,
        role: SessionRole,
    ) -> PortResult<()> {
        self.breaker
            .guard(async {
                self.with_workspace(workspace_id, move |db| {
                    existing_row(db, workspace_id)?;
                    let key = membership_key(workspace_id, session_id);
                    let added_at = match db.get(key.as_bytes())? {
                        Some(row) => decode_membership(&key, &row)?.added_at,
                        None => now_secs(),
                    };
                    let mut batch = WriteBatch::default();
                    put_membership(
                        &mut batch,
                        &MembershipRow::new(workspace_id, session_id, role, added_at),
                    )?;
                    db.write(batch)?;
                    Ok(())
                })
                .await
            })
            .await
    }

    /// Remove session from workspace association
    async fn remove_session_from_workspace(
        &self,
        workspace_id: Uuid,
        session_id: Uuid,
    ) -> PortResult<()> {
        self.breaker
            .guard(async {
                self.with_workspace(workspace_id, move |db| {
                    db.delete(membership_key(workspace_id, session_id).as_bytes())?;
                    Ok(())
                })
                .await
            })
            .await
    }
}
