// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Documents of modules and the [`DocumentStore`] trait implementation.
//!
//! Every version is a row of the `document` table keyed
//! `{workspace}_{id}_{version}`, unique on `(workspace_id, doc_id,
//! version)`; the latest version of each document has `latest` set. A
//! write creates the next version and clears `latest` on the one it read in
//! one transaction: of two writers that read the same latest version, the
//! second one's create or commit fails and is reported as a conflict. The order and
//! the keys of the latest versions change in the same transaction
//! ([`crate::surrealdb_storage::document_index`]).

use std::collections::HashMap;

use crate::port_error::PortResult;
use crate::port_error::Result;
use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize};

use surrealdb::types::{RecordId, SurrealValue};
use uuid::Uuid;

use post_cortex_core::core::document::{
    Document, DocumentCursor, DocumentData, DocumentDraft, DocumentKey, DocumentKind,
};
use post_cortex_ports::error::{corrupt, parse_name, parse_timestamp};
use post_cortex_ports::storage::{DocumentStore, DocumentWrite, LatestPage, LatestQuery};

use crate::document_order::{Window, window};
use crate::surrealdb_storage::SurrealDBStorage;
use crate::surrealdb_storage::document_index::{REINDEX, Reindex, UNINDEX, pair};
use crate::surrealdb_storage::limit::Limit;

/// Row of the `document` table.
#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
pub struct DocumentRecord {
    workspace_id: String,
    doc_id: String,
    kind: String,
    version: i64,
    /// The data's stored JSON: a message's type and base64 encoding, or
    /// JSON written before W23.
    data: String,
    title: String,
    text: String,
    updated_at: String,
    author: Option<String>,
    latest: bool,
}

impl DocumentRecord {
    fn from_document(document: &Document, latest: bool) -> Result<Self> {
        Ok(Self {
            workspace_id: document.workspace_id.to_string(),
            doc_id: document.id.to_string(),
            kind: document.kind.to_string(),
            version: i64::try_from(document.version)?,
            data: serde_json::to_string(&document.data)?,
            title: document.title.clone(),
            text: document.text.clone(),
            updated_at: document.updated_at.to_rfc3339(),
            author: document.author.clone(),
            latest,
        })
    }

    /// The document of this row.
    pub fn into_document(self) -> Result<Document> {
        let key = format!("{}_{}_{}", self.workspace_id, self.doc_id, self.version);
        let data: DocumentData =
            serde_json::from_str(&self.data).map_err(|e| corrupt("document", &key, e))?;
        Ok(Document {
            workspace_id: parse_name("document", &key, &self.workspace_id)?,
            id: parse_name("document", &key, &self.doc_id)?,
            kind: parse_name::<DocumentKind>("document", &key, &self.kind)?,
            version: u64::try_from(self.version)?,
            data,
            title: self.title,
            text: self.text,
            updated_at: parse_timestamp("document", &key, &self.updated_at)?,
            author: self.author,
        })
    }
}

/// `document` row for the batched upsert of an import.
#[derive(Serialize, SurrealValue)]
struct KeyedDocument {
    /// `{workspace}_{id}_{version}`.
    key: String,
    record: DocumentRecord,
}

fn row_key(workspace_id: Uuid, id: Uuid, version: u64) -> String {
    format!("{workspace_id}_{id}_{version}")
}

impl SurrealDBStorage {
    async fn document_rows(&self, query: &str, binds: DocumentBinds) -> Result<Vec<Document>> {
        let mut response = self
            .db()
            .query(query)
            .bind(("workspace_id", binds.workspace_id.to_string()))
            .bind(("doc_id", binds.id.map(|id| id.to_string())))
            .bind(("kind", binds.kind.map(|kind| kind.to_string())))
            .bind(("version", binds.version.map(i64::try_from).transpose()?))
            .bind(("limit", binds.limit.rows()))
            .await?
            .check()?;
        let rows: Vec<DocumentRecord> = response.take(0)?;
        rows.into_iter()
            .map(DocumentRecord::into_document)
            .collect()
    }

    async fn latest_document(&self, workspace_id: Uuid, id: Uuid) -> Result<Option<Document>> {
        let binds = DocumentBinds::of(workspace_id, Some(id));
        Ok(self
            .document_rows(
                "SELECT * OMIT id FROM document \
                 WHERE workspace_id = $workspace_id AND doc_id = $doc_id AND latest = true",
                binds,
            )
            .await?
            .into_iter()
            .next())
    }
}

/// The values a document query binds; unset ones bind `NONE`.
struct DocumentBinds {
    workspace_id: Uuid,
    id: Option<Uuid>,
    kind: Option<DocumentKind>,
    version: Option<u64>,
    limit: Limit,
}

impl DocumentBinds {
    fn of(workspace_id: Uuid, id: Option<Uuid>) -> Self {
        Self {
            workspace_id,
            id,
            kind: None,
            version: None,
            limit: Limit::of(usize::MAX),
        }
    }
}

#[async_trait]
impl DocumentStore for SurrealDBStorage {
    async fn get_document(
        &self,
        workspace_id: Uuid,
        id: Uuid,
        version: Option<u64>,
    ) -> PortResult<Option<Document>> {
        self.breaker
            .guard(async {
                let Some(version) = version else {
                    return self.latest_document(workspace_id, id).await;
                };
                let binds = DocumentBinds {
                    version: Some(version),
                    ..DocumentBinds::of(workspace_id, Some(id))
                };
                Ok(self
                    .document_rows(
                        "SELECT * OMIT id FROM document WHERE workspace_id = $workspace_id \
                         AND doc_id = $doc_id AND version = $version",
                        binds,
                    )
                    .await?
                    .into_iter()
                    .next())
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
                let (workspace_id, id) = (draft.workspace_id, draft.id);
                let latest = self.latest_document(workspace_id, id).await?;
                let stored = latest.as_ref().map_or(0, |d| d.version);
                let same_kind = latest.as_ref().is_none_or(|d| d.kind == draft.kind);
                if stored != expected_version || !same_kind {
                    return Ok(DocumentWrite::Conflict(latest));
                }
                let (document, keys) = draft.into_version(stored + 1, Utc::now());
                let reindex = Reindex::of(&document, Some(&keys))?;
                let db = self.db();
                let query = db.query(format!(
                    "BEGIN TRANSACTION; \
                     CREATE type::record('document', $key) CONTENT $record RETURN NONE; \
                     UPDATE type::record('document', $previous) SET latest = false \
                     RETURN NONE; {REINDEX} \
                     COMMIT TRANSACTION;"
                ));
                let created = reindex
                    .bind(query)
                    .bind(("key", row_key(workspace_id, id, document.version)))
                    .bind(("previous", row_key(workspace_id, id, stored)))
                    .bind(("record", DocumentRecord::from_document(&document, true)?))
                    .await?;
                match crate::surrealdb_storage::transaction::committed(created) {
                    Ok(()) => Ok(DocumentWrite::Written(document)),
                    // Another writer stored this version first.
                    Err(e) if crate::surrealdb_storage::transaction::lost_race(&e) => Ok(
                        DocumentWrite::Conflict(self.latest_document(workspace_id, id).await?),
                    ),
                    Err(e) => Err(e.into()),
                }
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
                let binds = DocumentBinds {
                    limit: Limit::of(limit),
                    ..DocumentBinds::of(workspace_id, Some(id))
                };
                let query = format!(
                    "SELECT * OMIT id FROM document \
                     WHERE workspace_id = $workspace_id AND doc_id = $doc_id \
                     ORDER BY version DESC {}",
                    binds.limit.clause()
                );
                self.document_rows(&query, binds).await
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
                let binds = DocumentBinds {
                    kind: kind.cloned(),
                    ..DocumentBinds::of(workspace_id, None)
                };
                // Two statements, not `($kind = NONE OR kind = $kind)`: with
                // the OR the index narrows by workspace and `latest` only, and
                // every latest document of the workspace is read and filtered.
                let query = match kind {
                    Some(_) => {
                        "SELECT * OMIT id FROM document \
                         WHERE workspace_id = $workspace_id AND latest = true AND kind = $kind"
                    }
                    None => {
                        "SELECT * OMIT id FROM document \
                         WHERE workspace_id = $workspace_id AND latest = true"
                    }
                };
                self.document_rows(query, binds).await
            })
            .await
    }

    async fn delete_document(&self, workspace_id: Uuid, id: Uuid) -> PortResult<bool> {
        self.breaker
            .guard(async {
                let (ws, doc_id) = (workspace_id.to_string(), id.to_string());
                let mut response = self
                    .db()
                    .query(format!(
                        "BEGIN TRANSACTION; \
                         DELETE document WHERE workspace_id = $workspace_id \
                         AND doc_id = $doc_id RETURN BEFORE; {UNINDEX} \
                         COMMIT TRANSACTION;"
                    ))
                    .bind(("pair", pair(&ws, &doc_id)))
                    .bind(("workspace_id", ws))
                    .bind(("doc_id", doc_id))
                    .await?
                    .check()?;
                // 0 is BEGIN.
                let removed: Vec<DocumentRecord> = response.take(1)?;
                Ok(!removed.is_empty())
            })
            .await
    }

    async fn latest_page(&self, query: LatestQuery<'_>) -> PortResult<LatestPage> {
        self.breaker
            .guard(async {
                let matched = self
                    .matching(&query)
                    .await?
                    .into_iter()
                    .map(|(inv, id, version)| (inv, id, (id, version)))
                    .collect();
                let Window { page, total, more } =
                    window(matched, query.after.as_ref(), query.limit)?;
                let records: Vec<RecordId> = page
                    .iter()
                    .map(|(id, version)| {
                        RecordId::new("document", row_key(query.workspace_id, *id, *version))
                    })
                    .collect();
                let mut response = self
                    .db()
                    .query("SELECT * OMIT id FROM $records")
                    .bind(("records", records))
                    .await?
                    .check()?;
                let rows: Vec<DocumentRecord> = response.take(0)?;
                let mut by_id: HashMap<Uuid, Document> = rows
                    .into_iter()
                    .map(|row| row.into_document().map(|d| (d.id, d)))
                    .collect::<Result<_>>()?;
                let documents = page
                    .iter()
                    .map(|(id, version)| {
                        by_id.remove(id).ok_or_else(|| {
                            corrupt(
                                "document order",
                                row_key(query.workspace_id, *id, *version),
                                "names a version that is not stored",
                            )
                            .into()
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                let next = documents.last().filter(|_| more).map(DocumentCursor::of);
                Ok(LatestPage {
                    documents,
                    total,
                    next,
                })
            })
            .await
    }

    async fn unkeyed_documents(&self, workspace_id: Uuid) -> PortResult<Vec<Document>> {
        self.breaker
            .guard(async {
                let mut documents = Vec::new();
                for id in self.unkeyed_ids(workspace_id).await? {
                    if let Some(document) = self.latest_document(workspace_id, id).await? {
                        documents.push(document);
                    }
                }
                Ok(documents)
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
                let Some(latest) = self.latest_document(workspace_id, id).await? else {
                    return Ok(false);
                };
                if latest.version != version {
                    return Ok(false);
                }
                // The version is checked again inside the transaction: a
                // write between the read and here keys its own version.
                let db = self.db();
                let query = db.query(format!(
                    "BEGIN TRANSACTION; \
                     IF (SELECT VALUE version FROM ONLY type::record('document_order', $pair)) \
                         = $version {{ {REINDEX} true }} ELSE {{ false }}; \
                     COMMIT TRANSACTION;"
                ));
                let mut response = Reindex::of(&latest, Some(&keys))?
                    .bind(query)
                    .bind(("version", i64::try_from(version)?))
                    .await?
                    .check()?;
                // 0 is BEGIN.
                let keyed: Option<bool> = response.take(1)?;
                Ok(keyed == Some(true))
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
                let stored = if replace {
                    None
                } else {
                    self.latest_document(workspace_id, id).await?
                };
                let newest = versions
                    .iter()
                    .chain(stored.as_ref())
                    .max_by_key(|d| d.version)
                    .map(|d| Document {
                        workspace_id,
                        id,
                        ..d.clone()
                    });
                let rows = versions
                    .into_iter()
                    .map(|document| {
                        let document = Document {
                            workspace_id,
                            id,
                            ..document
                        };
                        Ok(KeyedDocument {
                            key: row_key(workspace_id, id, document.version),
                            record: DocumentRecord::from_document(&document, false)?,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                let clear = if replace {
                    "DELETE document WHERE workspace_id = $workspace_id AND doc_id = $doc_id;"
                } else {
                    ""
                };
                let (ws, doc_id) = (workspace_id.to_string(), id.to_string());
                // The highest stored version becomes the latest, in its
                // order with `keys`; none left: out of the order.
                let (index, reindex) = match &newest {
                    Some(newest) => (REINDEX, Some(Reindex::of(newest, keys.as_deref())?)),
                    None => (UNINDEX, None),
                };
                let db = self.db();
                let query = db.query(format!(
                    "BEGIN TRANSACTION; {clear} \
                     FOR $d IN $rows {{ UPSERT type::record('document', $d.key) CONTENT $d.record; }}; \
                     LET $top = math::max((SELECT VALUE version FROM document \
                         WHERE workspace_id = $workspace_id AND doc_id = $doc_id)); \
                     UPDATE document SET latest = (version = $top) \
                         WHERE workspace_id = $workspace_id AND doc_id = $doc_id; {index} \
                     COMMIT TRANSACTION;"
                ));
                let query = match reindex {
                    Some(reindex) => reindex.bind(query),
                    None => query
                        .bind(("pair", pair(&ws, &doc_id)))
                        .bind(("workspace_id", ws))
                        .bind(("doc_id", doc_id)),
                };
                query.bind(("rows", rows)).await?.check()?;
                Ok(())
            })
            .await
    }
}
