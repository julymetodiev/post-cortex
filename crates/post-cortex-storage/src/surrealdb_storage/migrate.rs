// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Versioned data migrations, run once when the schema is initialised.
//!
//! The data version lives in `pcx_meta:schema` (`version`, an int; a
//! database without it is version 0). [`SurrealDBStorage::migrate`] runs
//! every step above the stored version in order and records the new
//! version after each, so a migration interrupted half way repeats only
//! its unfinished step. Each step is idempotent.
//!
//! - **1**: edge record ids become [`SurrealDBStorage::relation_id`]
//!   (version 1 of the edge identity, built from wire names). Edges stored
//!   under the old `DefaultHasher` ids are re-keyed; edges that map to one
//!   id (duplicates) merge into one; edges whose endpoint entity is gone
//!   (orphans the loader skips) are deleted.
//! - **2**: the HNSW index `idx_embedding_hnsw` is removed. Search runs on
//!   the memory layer's in-memory index; nothing read the database index,
//!   and every embedding write paid for maintaining it.
//! - **3**: every document's latest version gets its place in its kind's
//!   order, unkeyed, for memory to key at start (docs/memory.md, document
//!   keys and pages).
//! - **4**: every workspace gets its slug, the oldest first
//!   ([`post_cortex_core::slug::workspace_slug`]); then the slug's unique
//!   index is defined (in the transaction of the writes it conflicts with
//!   them).
//! - **5**: every workspace forbids the remote reranker (a person allows
//!   it per workspace).
//!
//! [`SurrealDBStorage::open_existing`] never migrates: it changes nothing,
//! and reads do not depend on edge ids.

use crate::port_error::Result;
use serde::{Deserialize, Serialize};
use surrealdb::types::SurrealValue;
use tracing::info;
use uuid::Uuid;

use post_cortex_core::core::context_update::RelationType;
use post_cortex_core::slug::workspace_slug;

use crate::surrealdb_storage::document_index::{REINDEX, Reindex};
use crate::surrealdb_storage::documents::DocumentRecord;
use crate::surrealdb_storage::ids;
use crate::surrealdb_storage::records::deserialize_surreal_option;
use surrealdb::Surreal;
use surrealdb::engine::any::Any;

/// Data version this build writes.
pub const DATA_VERSION: i64 = 5;

/// An edge as stored, with its current key.
#[derive(Deserialize, SurrealValue)]
struct StoredEdge {
    key: String,
    #[serde(default, deserialize_with = "deserialize_surreal_option")]
    session_id: Option<String>,
    #[serde(default, deserialize_with = "deserialize_surreal_option")]
    from_entity: Option<String>,
    #[serde(default, deserialize_with = "deserialize_surreal_option")]
    to_entity: Option<String>,
    #[serde(default, deserialize_with = "deserialize_surreal_option")]
    context: Option<String>,
}

/// An edge to write under its version-1 key.
#[derive(Serialize, SurrealValue)]
struct Rekeyed {
    old: String,
    new: String,
    from_key: String,
    to_key: String,
    context: String,
    session_id: String,
}

/// The stored data version (0 before versions were recorded).
pub async fn data_version(db: &Surreal<Any>) -> Result<i64> {
    let mut response = db
        .query("SELECT VALUE version FROM ONLY pcx_meta:schema;")
        .await?
        .check()?;
    let version: Option<i64> = response.take(0)?;
    Ok(version.unwrap_or(0))
}

/// Run every migration step above the stored data version.
pub async fn migrate(db: &Surreal<Any>, name: &str) -> Result<()> {
    db.query("DEFINE TABLE IF NOT EXISTS pcx_meta SCHEMALESS;")
        .await?
        .check()?;
    let mut version = data_version(db).await?;
    if version > DATA_VERSION {
        return Err(post_cortex_ports::PortError::unavailable(format!(
            "SurrealDB database {name} holds data version {version}; this build reads up \
             to {DATA_VERSION}. Use a newer post-cortex."
        ))
        .into());
    }
    while version < DATA_VERSION {
        version += 1;
        match version {
            1 => rekey_edges(db).await?,
            2 => {
                db.query("REMOVE INDEX IF EXISTS idx_embedding_hnsw ON embedding;")
                    .await?
                    .check()?;
            }
            3 => {
                let placed = order_documents(db).await?;
                info!("SurrealDBStorage: {placed} documents ordered");
            }
            4 => {
                let slugged = slug_workspaces(db).await?;
                info!("SurrealDBStorage: {slugged} workspaces got a slug");
            }
            5 => {
                db.query("UPDATE workspace SET remote_rerank = false WHERE remote_rerank IS NONE;")
                    .await?
                    .check()?;
            }
            _ => unreachable!("every version up to DATA_VERSION has a step"),
        }
        db.query("UPSERT pcx_meta:schema SET version = $version;")
            .bind(("version", version))
            .await?
            .check()?;
        info!("SurrealDBStorage: migrated data to version {version}");
    }
    Ok(())
}

/// Step 1: every edge under its [`ids::relation_id`].
async fn rekey_edges(db: &Surreal<Any>) -> Result<()> {
    for rel in RelationType::ALL {
        let table = rel.as_str();
        let mut response = db
            .query(format!(
                "SELECT meta::id(id) AS key, session_id, in.name AS from_entity, \
                 out.name AS to_entity, context FROM {table};"
            ))
            .await?
            .check()?;
        let edges: Vec<StoredEdge> = response.take(0)?;
        let mut rekeyed = Vec::new();
        let mut orphans = Vec::new();
        for edge in edges {
            let session = edge.session_id.as_deref().map(Uuid::parse_str);
            let (Some(Ok(session)), Some(from), Some(to)) =
                (session, edge.from_entity, edge.to_entity)
            else {
                orphans.push(edge.key);
                continue;
            };
            let new = ids::relation_id(session, &from, &to, rel);
            if new != edge.key {
                rekeyed.push(Rekeyed {
                    old: edge.key,
                    new,
                    from_key: ids::entity_id(session, &from),
                    to_key: ids::entity_id(session, &to),
                    context: edge.context.unwrap_or_default(),
                    session_id: session.to_string(),
                });
            }
        }
        if rekeyed.is_empty() && orphans.is_empty() {
            continue;
        }
        let (n_rekeyed, n_orphans) = (rekeyed.len(), orphans.len());
        db.query(format!(
            "BEGIN TRANSACTION; \
                 FOR $r IN $rekeyed {{ \
                     DELETE type::record('{table}', $r.old); \
                     UPSERT type::record('{table}', $r.new) SET \
                         in = type::record('entity', $r.from_key), \
                         out = type::record('entity', $r.to_key), \
                         context = $r.context, session_id = $r.session_id; \
                 }}; \
                 FOR $key IN $orphans {{ DELETE type::record('{table}', $key); }}; \
                 COMMIT TRANSACTION;"
        ))
        .bind(("rekeyed", rekeyed))
        .bind(("orphans", orphans))
        .await?
        .check()?;
        info!("SurrealDBStorage: {table}: {n_rekeyed} edges re-keyed, {n_orphans} orphans deleted");
    }
    Ok(())
}

/// A workspace before its slug.
#[derive(Deserialize, SurrealValue)]
struct Unslugged {
    workspace_id: String,
    name: String,
    created_at: u64,
    #[serde(default, deserialize_with = "deserialize_surreal_option")]
    slug: Option<String>,
}

/// Step 4: the slug of every workspace without one, the oldest first; those
/// with one keep it. Then the unique index.
async fn slug_workspaces(db: &Surreal<Any>) -> Result<usize> {
    let mut response = db
        .query("SELECT workspace_id, name, created_at, slug FROM workspace;")
        .await?
        .check()?;
    let rows: Vec<Unslugged> = response.take(0)?;
    let mut taken: std::collections::HashSet<String> =
        rows.iter().filter_map(|r| r.slug.clone()).collect();
    let mut unslugged = Vec::new();
    for row in rows.into_iter().filter(|r| r.slug.is_none()) {
        let id = post_cortex_ports::error::parse_uuid(
            "workspace",
            &row.workspace_id,
            &row.workspace_id,
        )?;
        unslugged.push((row.created_at, id, row.name));
    }
    unslugged.sort();
    let slugged = unslugged.len();
    for (_, id, name) in unslugged {
        let slug = workspace_slug(&name, id, |base| taken.contains(base));
        taken.insert(slug.clone());
        db.query("UPDATE type::record('workspace', $id) SET slug = $slug;")
            .bind(("id", id.to_string()))
            .bind(("slug", slug))
            .await?
            .check()?;
    }
    db.query("DEFINE INDEX IF NOT EXISTS idx_workspace_slug ON workspace FIELDS slug UNIQUE;")
        .await?
        .check()?;
    Ok(slugged)
}

/// Step 3: every latest version already stored gets its place in its
/// kind's order, unkeyed, for memory to key at start. Repeating it unkeys
/// what it orders again; memory keys it again.
async fn order_documents(db: &Surreal<Any>) -> Result<usize> {
    let mut response = db
        .query("SELECT * OMIT id FROM document WHERE latest = true")
        .await?
        .check()?;
    let rows: Vec<DocumentRecord> = response.take(0)?;
    let placed = rows.len();
    for row in rows {
        let document = row.into_document()?;
        let query = db.query(format!("BEGIN TRANSACTION; {REINDEX} COMMIT TRANSACTION;"));
        Reindex::of(&document, None)?.bind(query).await?.check()?;
    }
    Ok(placed)
}

#[cfg(test)]
mod tests;
