// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Workspaces: their rows, projects and members.

use crate::port_error::PortResult;
use crate::port_error::Result;
use crate::surrealdb_storage::SurrealDBStorage;
use crate::surrealdb_storage::records::{
    WorkspaceProjectRecord, WorkspaceRecord, WorkspaceSessionRecord,
};
use async_trait::async_trait;
use post_cortex_core::workspace::{SessionRole, WorkspaceProject};
use post_cortex_ports::error::{parse_name, parse_uuid};
use post_cortex_ports::storage::StoredWorkspace;
use post_cortex_ports::storage::WorkspaceStore;
use serde::Serialize;
use std::collections::HashMap;
use surrealdb::types::SurrealValue;
use tracing::debug;
use uuid::Uuid;

/// Unix seconds now.
fn now_secs() -> u64 {
    chrono::Utc::now().timestamp().max(0) as u64
}

fn project_records(projects: &[WorkspaceProject]) -> Vec<WorkspaceProjectRecord> {
    projects
        .iter()
        .map(|p| WorkspaceProjectRecord {
            key: p.key.clone(),
            session_id: p.session_id.to_string(),
        })
        .collect()
}

/// A membership row keyed `{workspace}_{session}`; the role is stored by its
/// stored (variant) name.
fn membership(workspace_id: Uuid, session_id: Uuid, role: SessionRole, at: u64) -> KeyedMembership {
    KeyedMembership {
        key: format!("{}_{}", workspace_id, session_id),
        record: WorkspaceSessionRecord {
            workspace_id: workspace_id.to_string(),
            session_id: session_id.to_string(),
            role: role.stored_name().to_string(),
            added_at: at,
        },
    }
}

/// Workspace membership row for the batched upsert.
#[derive(Serialize, SurrealValue)]
struct KeyedMembership {
    key: String,
    record: WorkspaceSessionRecord,
}

#[async_trait]
impl WorkspaceStore for SurrealDBStorage {
    /// Workspace row and the new memberships in one query. A new row
    /// forbids the remote reranker (the field's default); an existing row
    /// keeps its slug, creation time, projects, archive state and remote
    /// reranking; existing members keep their role.
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
                debug!(
                    "SurrealDBStorage: Saving workspace {} ({})",
                    name, workspace_id
                );
                let now = now_secs();
                let members: Vec<KeyedMembership> = session_ids
                    .iter()
                    .map(|session_id| {
                        membership(workspace_id, *session_id, SessionRole::Primary, now)
                    })
                    .collect();
                self.db()
                    .query(
                        "UPSERT type::record('workspace', $workspace_id) SET \
                 workspace_id = $workspace_id, name = $name, slug = slug ?? $slug, \
                 description = $description, created_at = created_at ?? $now; \
                 FOR $m IN $members { \
                 UPSERT type::record('workspace_session', $m.key) SET \
                 workspace_id = $m.record.workspace_id, session_id = $m.record.session_id, \
                 role = role ?? $m.record.role, added_at = added_at ?? $m.record.added_at; };",
                    )
                    .bind(("workspace_id", workspace_id.to_string()))
                    .bind(("name", name.to_string()))
                    .bind(("slug", slug.to_string()))
                    .bind(("description", description.to_string()))
                    .bind(("now", now))
                    .bind(("members", members))
                    .await?
                    .check()?;
                Ok(())
            })
            .await
    }

    async fn save_workspace_projects(
        &self,
        workspace_id: Uuid,
        projects: &[WorkspaceProject],
    ) -> PortResult<()> {
        self.breaker
            .guard(async {
                let projects = project_records(projects);
                let updated: Vec<WorkspaceRecord> = self
                    .db()
                    .query(
                        "UPDATE type::record('workspace', $workspace_id) SET projects = $projects;",
                    )
                    .bind(("workspace_id", workspace_id.to_string()))
                    .bind(("projects", projects))
                    .await?
                    .check()?
                    .take(0)?;
                if updated.is_empty() {
                    return Err(post_cortex_ports::error::workspace_not_found(workspace_id).into());
                }
                Ok(())
            })
            .await
    }

    async fn update_workspace(
        &self,
        workspace_id: Uuid,
        update: &post_cortex_ports::storage::WorkspaceUpdate,
    ) -> PortResult<()> {
        self.breaker
            .guard(async {
                let mut sets = Vec::new();
                if update.name.is_some() {
                    sets.push("name = $name");
                }
                if update.description.is_some() {
                    sets.push("description = $description");
                }
                match update.archived_at {
                    Some(Some(_)) => sets.push("archived_at = $archived_at"),
                    Some(None) => sets.push("archived_at = NONE"),
                    None => {}
                }
                if update.remote_rerank.is_some() {
                    sets.push("remote_rerank = $remote_rerank");
                }
                if sets.is_empty() {
                    // Nothing to change; still report a missing workspace.
                    sets.push("workspace_id = workspace_id");
                }
                let updated: Vec<WorkspaceRecord> = self
                    .db()
                    .query(format!(
                        "UPDATE type::record('workspace', $workspace_id) SET {};",
                        sets.join(", ")
                    ))
                    .bind(("workspace_id", workspace_id.to_string()))
                    .bind(("name", update.name.clone().unwrap_or_default()))
                    .bind((
                        "description",
                        update.description.clone().unwrap_or_default(),
                    ))
                    .bind((
                        "archived_at",
                        update.archived_at.flatten().map(|t| t as i64).unwrap_or(0),
                    ))
                    .bind(("remote_rerank", update.remote_rerank.unwrap_or_default()))
                    .await?
                    .check()?
                    .take(0)?;
                if updated.is_empty() {
                    return Err(post_cortex_ports::error::workspace_not_found(workspace_id).into());
                }
                Ok(())
            })
            .await
    }

    /// One transaction.
    async fn restore_workspace(&self, workspace: &StoredWorkspace) -> PortResult<()> {
        self.breaker
            .guard(async {
                let record = WorkspaceRecord {
                    workspace_id: workspace.id.to_string(),
                    name: workspace.name.clone(),
                    slug: workspace.slug.clone(),
                    description: workspace.description.clone(),
                    created_at: workspace.created_at,
                    projects: Some(project_records(&workspace.projects)),
                    archived_at: workspace.archived_at,
                    remote_rerank: workspace.remote_rerank,
                };
                let now = now_secs();
                let members: Vec<KeyedMembership> = workspace
                    .sessions
                    .iter()
                    .map(|(session_id, role)| membership(workspace.id, *session_id, *role, now))
                    .collect();
                let keep: Vec<String> = workspace
                    .sessions
                    .iter()
                    .map(|(id, _)| id.to_string())
                    .collect();
                self.db()
                    .query(
                        "BEGIN TRANSACTION; \
                 UPSERT type::record('workspace', $workspace_id) CONTENT $record; \
                 DELETE workspace_session WHERE workspace_id = $workspace_id \
                 AND session_id NOT IN $keep; \
                 FOR $m IN $members { \
                 UPSERT type::record('workspace_session', $m.key) SET \
                 workspace_id = $m.record.workspace_id, session_id = $m.record.session_id, \
                 role = $m.record.role, added_at = added_at ?? $m.record.added_at; }; \
                 COMMIT TRANSACTION;",
                    )
                    .bind(("workspace_id", workspace.id.to_string()))
                    .bind(("record", record))
                    .bind(("keep", keep))
                    .bind(("members", members))
                    .await?
                    .check()?;
                Ok(())
            })
            .await
    }

    async fn delete_workspace(&self, workspace_id: Uuid) -> PortResult<()> {
        self.breaker
            .guard(async {
                debug!("SurrealDBStorage: Deleting workspace {}", workspace_id);

                self.db()
                    .query(
                        "DELETE type::record('workspace', $workspace_id); \
                 DELETE workspace_session WHERE workspace_id = $workspace_id; \
                 DELETE radar_dismissal WHERE workspace_id = $workspace_id; \
                 DELETE card WHERE scope = $card_scope; \
                 DELETE document WHERE workspace_id = $workspace_id; \
                 DELETE document_order WHERE workspace_id = $workspace_id; \
                 DELETE document_key WHERE workspace_id = $workspace_id; \
                 DELETE document_unkeyed WHERE workspace_id = $workspace_id;",
                    )
                    .bind(("workspace_id", workspace_id.to_string()))
                    .bind((
                        "card_scope",
                        post_cortex_core::core::card::CardScope::Workspace(workspace_id).key(),
                    ))
                    .await?
                    .check()?;

                debug!("SurrealDBStorage: Workspace deleted successfully");

                Ok(())
            })
            .await
    }

    /// Workspaces and memberships in one query.
    async fn list_workspaces(&self) -> PortResult<Vec<StoredWorkspace>> {
        self.breaker
            .guard(async {
                debug!("SurrealDBStorage: Listing workspaces");

                let mut response = self
                    .db()
                    .query("SELECT * FROM workspace; SELECT * FROM workspace_session;")
                    .await?;
                let records: Vec<WorkspaceRecord> = response.take(0)?;
                let memberships: Vec<WorkspaceSessionRecord> = response.take(1)?;

                let mut sessions_by_workspace: HashMap<String, Vec<(Uuid, SessionRole)>> =
                    HashMap::new();
                for m in memberships {
                    const KIND: &str = "workspace membership";
                    let session_id = parse_uuid(KIND, &m.session_id, &m.session_id)?;
                    let role = parse_name(KIND, session_id, &m.role)?;
                    sessions_by_workspace
                        .entry(m.workspace_id)
                        .or_default()
                        .push((session_id, role));
                }

                let mut workspaces = Vec::with_capacity(records.len());
                for record in records {
                    const KIND: &str = "workspace";
                    let id = parse_uuid(KIND, &record.workspace_id, &record.workspace_id)?;
                    let projects = record
                        .projects
                        .unwrap_or_default()
                        .into_iter()
                        .map(|p| {
                            Ok(WorkspaceProject {
                                session_id: parse_uuid(KIND, id, &p.session_id)?,
                                key: p.key,
                            })
                        })
                        .collect::<Result<Vec<_>>>()?;
                    workspaces.push(StoredWorkspace {
                        id,
                        sessions: sessions_by_workspace
                            .remove(&record.workspace_id)
                            .unwrap_or_default(),
                        name: record.name,
                        slug: record.slug,
                        description: record.description,
                        created_at: record.created_at,
                        projects,
                        archived_at: record.archived_at,
                        remote_rerank: record.remote_rerank,
                    });
                }

                debug!("SurrealDBStorage: Listed {} workspaces", workspaces.len());

                Ok(workspaces)
            })
            .await
    }

    async fn add_session_to_workspace(
        &self,
        workspace_id: Uuid,
        session_id: Uuid,
        role: SessionRole,
    ) -> PortResult<()> {
        self.breaker
            .guard(async {
                debug!(
                    "SurrealDBStorage: Adding session {} to workspace {} with role {:?}",
                    session_id, workspace_id, role
                );

                let mut response = self
            .db()
            .query("SELECT VALUE workspace_id FROM ONLY type::record('workspace', $workspace_id);")
            .bind(("workspace_id", workspace_id.to_string()))
            .await?
            .check()?;
                let exists: Option<String> = response.take(0)?;
                if exists.is_none() {
                    return Err(post_cortex_ports::error::workspace_not_found(workspace_id).into());
                }
                let m = membership(workspace_id, session_id, role, now_secs());
                self.db()
                    .query(
                        "UPSERT type::record('workspace_session', $m.key) SET \
                 workspace_id = $m.record.workspace_id, session_id = $m.record.session_id, \
                 role = $m.record.role, added_at = added_at ?? $m.record.added_at;",
                    )
                    .bind(("m", m))
                    .await?
                    .check()?;
                Ok(())
            })
            .await
    }

    async fn remove_session_from_workspace(
        &self,
        workspace_id: Uuid,
        session_id: Uuid,
    ) -> PortResult<()> {
        self.breaker
            .guard(async {
                debug!(
                    "SurrealDBStorage: Removing session {} from workspace {}",
                    session_id, workspace_id
                );

                let key = format!("{}_{}", workspace_id, session_id);
                let _: Option<WorkspaceSessionRecord> = self
                    .db()
                    .delete(("workspace_session", key.as_str()))
                    .await?;

                Ok(())
            })
            .await
    }
}
