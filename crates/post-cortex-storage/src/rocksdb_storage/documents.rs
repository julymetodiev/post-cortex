// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Documents of modules and the [`DocumentStore`] trait implementation.
//!
//! Key families (see [`crate::rocksdb_storage::rows`]):
//! - `doc:{ws}:{id}:{version}` -> marked JSON [`Document`]; `version` 20
//!   zero-padded digits, so key order is version order.
//! - `doc_head:{ws}:{id}` -> the latest version (8 bytes, big endian).
//! - the order and the keys of the latest versions
//!   ([`crate::rocksdb_storage::document_index`]).
//!
//! A write is one `WriteBatch` under the lock of the document's id.

use crate::port_error::PortResult;
use crate::port_error::Result;
use async_trait::async_trait;
use chrono::Utc;
use rocksdb::{DB, Direction, IteratorMode, ReadOptions, WriteBatch};
use uuid::Uuid;

use post_cortex_core::core::document::{
    Document, DocumentCursor, DocumentDraft, DocumentKey, DocumentKind,
};
use post_cortex_ports::error::corrupt;
use post_cortex_ports::storage::{DocumentStore, DocumentWrite, LatestPage, LatestQuery};

use crate::document_order::{Window, window};
use crate::rocksdb_storage::RealRocksDBStorage;
use crate::rocksdb_storage::core::{blocking, keys_with_prefix};
use crate::rocksdb_storage::document_index::{
    self, ids_under, index, matching, order_prefix, unindex, unkeyed_prefix,
};
use crate::rocksdb_storage::rows::{DOCUMENT_V1, decode_json_marked, json_marked};

fn version_prefix(ws: Uuid, id: Uuid) -> String {
    format!("doc:{ws}:{id}:")
}

fn version_key(ws: Uuid, id: Uuid, version: u64) -> String {
    format!("doc:{ws}:{id}:{version:020}")
}

fn head_prefix(ws: Uuid) -> String {
    format!("doc_head:{ws}:")
}

fn head_key(ws: Uuid, id: Uuid) -> String {
    format!("doc_head:{ws}:{id}")
}

fn decode_document(key: &[u8], row: &[u8]) -> Result<Document> {
    decode_json_marked("document", &String::from_utf8_lossy(key), DOCUMENT_V1, row)
}

fn decode_version(key: &str, bytes: &[u8]) -> Result<u64> {
    let raw: [u8; 8] = bytes
        .try_into()
        .map_err(|_| corrupt("document head", key, format!("{} bytes", bytes.len())))?;
    Ok(u64::from_be_bytes(raw))
}

fn head(db: &DB, ws: Uuid, id: Uuid) -> Result<u64> {
    let key = head_key(ws, id);
    match db.get(key.as_bytes())? {
        Some(bytes) => decode_version(&key, &bytes),
        None => Ok(0),
    }
}

fn load(db: &DB, ws: Uuid, id: Uuid, version: u64) -> Result<Option<Document>> {
    if version == 0 {
        return Ok(None);
    }
    let key = version_key(ws, id, version);
    db.get(key.as_bytes())?
        .map(|bytes| decode_document(key.as_bytes(), &bytes))
        .transpose()
}

/// The latest version of document `id` of `ws`.
fn load_latest(db: &DB, ws: Uuid, id: Uuid) -> Result<Option<Document>> {
    load(db, ws, id, head(db, ws, id)?)
}

/// The latest version of document `id`, which an index names.
fn indexed(db: &DB, ws: Uuid, id: Uuid) -> Result<Document> {
    load_latest(db, ws, id)?.ok_or_else(|| {
        corrupt(
            "document head",
            head_key(ws, id),
            "an index names a document without versions",
        )
        .into()
    })
}

/// Delete document `id` of `ws` with every version in `batch`; its latest
/// version (`None`: no such document).
fn delete_in(db: &DB, batch: &mut WriteBatch, ws: Uuid, id: Uuid) -> Result<Option<Document>> {
    let latest = load_latest(db, ws, id)?;
    if let Some(document) = &latest {
        unindex(db, batch, document)?;
    }
    batch.delete(head_key(ws, id).as_bytes());
    for key in keys_with_prefix(db, version_prefix(ws, id).as_bytes())? {
        batch.delete(&key);
    }
    Ok(latest)
}

/// Delete every document of `ws` with its versions in `batch`.
pub fn delete_documents_in(db: &DB, batch: &mut WriteBatch, ws: Uuid) -> Result<()> {
    let families = [format!("doc:{ws}:"), head_prefix(ws)];
    for prefix in families.iter().chain(document_index::prefixes(ws).iter()) {
        for key in keys_with_prefix(db, prefix.as_bytes())? {
            batch.delete(&key);
        }
    }
    Ok(())
}

fn put_version(batch: &mut WriteBatch, document: &Document) -> Result<()> {
    let (ws, id) = (document.workspace_id, document.id);
    batch.put(
        version_key(ws, id, document.version).as_bytes(),
        json_marked(DOCUMENT_V1, document)?,
    );
    Ok(())
}

fn put_head(batch: &mut WriteBatch, document: &Document) {
    let (ws, id) = (document.workspace_id, document.id);
    batch.put(head_key(ws, id).as_bytes(), document.version.to_be_bytes());
}

fn history(db: &DB, ws: Uuid, id: Uuid, limit: usize) -> Result<Vec<Document>> {
    let prefix = version_prefix(ws, id);
    let mut opts = ReadOptions::default();
    opts.set_total_order_seek(true);
    opts.set_iterate_lower_bound(prefix.clone().into_bytes());
    // Seek from just past the prefix, walking back.
    let mut upper = prefix.clone().into_bytes();
    upper.push(0xFF);
    let mut documents = Vec::new();
    for item in db.iterator_opt(IteratorMode::From(&upper, Direction::Reverse), opts) {
        if documents.len() >= limit {
            break;
        }
        let (key, value) = item?;
        if !key.starts_with(prefix.as_bytes()) {
            break;
        }
        documents.push(decode_document(&key, &value)?);
    }
    Ok(documents)
}

fn latest(db: &DB, ws: Uuid, kind: Option<&DocumentKind>) -> Result<Vec<Document>> {
    let ids = match kind {
        Some(kind) => ids_under(db, &order_prefix(ws, kind))?,
        None => ids_under(db, &head_prefix(ws))?,
    };
    ids.into_iter().map(|id| indexed(db, ws, id)).collect()
}

fn page(db: &DB, query: &LatestQuery<'_>) -> Result<LatestPage> {
    let Window { page, total, more } =
        window(matching(db, query)?, query.after.as_ref(), query.limit)?;
    let documents = page
        .into_iter()
        .map(|id| indexed(db, query.workspace_id, id))
        .collect::<Result<Vec<_>>>()?;
    let next = documents.last().filter(|_| more).map(DocumentCursor::of);
    Ok(LatestPage {
        documents,
        total,
        next,
    })
}

/// Data version 4: the documents already stored in their kinds' order,
/// unkeyed.
pub fn order_existing(db: &DB) -> Result<usize> {
    document_index::order_existing(db, load_latest)
}

#[async_trait]
impl DocumentStore for RealRocksDBStorage {
    async fn get_document(
        &self,
        workspace_id: Uuid,
        id: Uuid,
        version: Option<u64>,
    ) -> PortResult<Option<Document>> {
        self.breaker
            .guard(async {
                let db = self.db.clone();
                blocking(move || {
                    let version = match version {
                        Some(version) => version,
                        None => head(&db, workspace_id, id)?,
                    };
                    load(&db, workspace_id, id, version)
                })
                .await
            })
            .await
    }

    async fn put_document(
        &self,
        draft: DocumentDraft,
        expected_version: u64,
    ) -> PortResult<DocumentWrite> {
        self.breaker
            .guard(async {
                let db = self.db.clone();
                let locks = self.session_locks.clone();
                blocking(move || {
                    let (ws, id) = (draft.workspace_id, draft.id);
                    let _guard = locks.lock(id);
                    let latest = load_latest(&db, ws, id)?;
                    let stored = latest.as_ref().map_or(0, |d| d.version);
                    let same_kind = latest.as_ref().is_none_or(|d| d.kind == draft.kind);
                    if stored != expected_version || !same_kind {
                        return Ok(DocumentWrite::Conflict(latest));
                    }
                    let (document, keys) = draft.into_version(stored + 1, Utc::now());
                    let mut batch = WriteBatch::default();
                    if let Some(previous) = &latest {
                        unindex(&db, &mut batch, previous)?;
                    }
                    put_version(&mut batch, &document)?;
                    put_head(&mut batch, &document);
                    index(&mut batch, &document, Some(&keys))?;
                    db.write(batch)?;
                    Ok(DocumentWrite::Written(document))
                })
                .await
            })
            .await
    }

    async fn document_history(
        &self,
        workspace_id: Uuid,
        id: Uuid,
        limit: usize,
    ) -> PortResult<Vec<Document>> {
        self.breaker
            .guard(async {
                let db = self.db.clone();
                blocking(move || history(&db, workspace_id, id, limit)).await
            })
            .await
    }

    async fn latest_documents(
        &self,
        workspace_id: Uuid,
        kind: Option<&DocumentKind>,
    ) -> PortResult<Vec<Document>> {
        self.breaker
            .guard(async {
                let db = self.db.clone();
                let kind = kind.cloned();
                blocking(move || latest(&db, workspace_id, kind.as_ref())).await
            })
            .await
    }

    async fn latest_page(&self, query: LatestQuery<'_>) -> PortResult<LatestPage> {
        self.breaker
            .guard(async {
                let db = self.db.clone();
                let (kind, keys) = (query.kind.clone(), query.keys.to_vec());
                let (workspace_id, after, limit) = (query.workspace_id, query.after, query.limit);
                blocking(move || {
                    page(
                        &db,
                        &LatestQuery {
                            workspace_id,
                            kind: &kind,
                            keys: &keys,
                            after,
                            limit,
                        },
                    )
                })
                .await
            })
            .await
    }

    async fn unkeyed_documents(&self, workspace_id: Uuid) -> PortResult<Vec<Document>> {
        self.breaker
            .guard(async {
                let db = self.db.clone();
                blocking(move || {
                    ids_under(&db, &unkeyed_prefix(workspace_id))?
                        .into_iter()
                        .map(|id| indexed(&db, workspace_id, id))
                        .collect()
                })
                .await
            })
            .await
    }

    async fn set_document_keys(
        &self,
        workspace_id: Uuid,
        id: Uuid,
        version: u64,
        keys: Vec<DocumentKey>,
    ) -> PortResult<bool> {
        self.breaker
            .guard(async {
                let db = self.db.clone();
                let locks = self.session_locks.clone();
                blocking(move || {
                    let _guard = locks.lock(id);
                    let Some(latest) = load_latest(&db, workspace_id, id)? else {
                        return Ok(false);
                    };
                    if latest.version != version {
                        return Ok(false);
                    }
                    let mut batch = WriteBatch::default();
                    unindex(&db, &mut batch, &latest)?;
                    index(&mut batch, &latest, Some(&keys))?;
                    db.write(batch)?;
                    Ok(true)
                })
                .await
            })
            .await
    }

    async fn delete_document(&self, workspace_id: Uuid, id: Uuid) -> PortResult<bool> {
        self.breaker
            .guard(async {
                let db = self.db.clone();
                let locks = self.session_locks.clone();
                blocking(move || {
                    let _guard = locks.lock(id);
                    let mut batch = WriteBatch::default();
                    let existed = delete_in(&db, &mut batch, workspace_id, id)?.is_some();
                    db.write(batch)?;
                    Ok(existed)
                })
                .await
            })
            .await
    }

    async fn restore_document_versions(
        &self,
        workspace_id: Uuid,
        id: Uuid,
        versions: Vec<Document>,
        replace: bool,
        keys: Option<Vec<DocumentKey>>,
    ) -> PortResult<()> {
        self.breaker
            .guard(async {
                if versions.is_empty() && !replace {
                    return Ok(());
                }
                let db = self.db.clone();
                let locks = self.session_locks.clone();
                blocking(move || {
                    let _guard = locks.lock(id);
                    let mut batch = WriteBatch::default();
                    let mut newest = if replace {
                        delete_in(&db, &mut batch, workspace_id, id)?;
                        None
                    } else {
                        let stored = load_latest(&db, workspace_id, id)?;
                        if let Some(previous) = &stored {
                            unindex(&db, &mut batch, previous)?;
                        }
                        stored
                    };
                    for mut document in versions {
                        document.workspace_id = workspace_id;
                        document.id = id;
                        put_version(&mut batch, &document)?;
                        if newest.as_ref().is_none_or(|n| n.version < document.version) {
                            newest = Some(document);
                        }
                    }
                    if let Some(newest) = &newest {
                        put_head(&mut batch, newest);
                        index(&mut batch, newest, keys.as_deref())?;
                    }
                    db.write(batch)?;
                    Ok(())
                })
                .await
            })
            .await
    }
}
