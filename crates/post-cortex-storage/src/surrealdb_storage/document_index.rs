// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The order and the keys of the latest versions of documents
//! (docs/memory.md, document keys and pages): small rows beside
//! `document`, read through indexes on the fields a page names and ordered
//! here.
//! - `document_order:{ws}_{id}`: the latest version's kind, version and
//!   inverted time ([`crate::document_order`]);
//! - `document_key:{ws}_{id}_{key}`: one per key of the latest version;
//! - `document_unkeyed:{ws}_{id}`: the latest version has no keys (its kind,
//!   version and inverted time); a page with keys holds them too, for
//!   memory to check.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use surrealdb::types::{RecordId, SurrealValue};
use uuid::Uuid;

use post_cortex_core::core::document::{Document, DocumentKey};
use post_cortex_ports::storage::LatestQuery;

use crate::document_order::inverted;
use crate::port_error::Result;
use crate::surrealdb_storage::SurrealDBStorage;

/// What puts a latest version in its order with its keys, the old ones
/// gone: run in the transaction that writes it, after its binds
/// ([`Reindex::bind`]).
pub const REINDEX: &str = "DELETE document_key WHERE workspace_id = $workspace_id \
         AND doc_id = $doc_id; \
     DELETE type::record('document_unkeyed', $pair); \
     UPSERT type::record('document_order', $pair) CONTENT $order RETURN NONE; \
     FOR $k IN $key_rows { UPSERT type::record('document_key', $k.key) CONTENT $k.row RETURN NONE; }; \
     FOR $u IN $unkeyed_rows { UPSERT type::record('document_unkeyed', $pair) CONTENT $u RETURN NONE; };";

/// What takes a document out of its order and keys: binds `$workspace_id`,
/// `$doc_id` and `$pair`.
pub const UNINDEX: &str = "DELETE document_key WHERE workspace_id = $workspace_id \
         AND doc_id = $doc_id; \
     DELETE type::record('document_unkeyed', $pair); \
     DELETE type::record('document_order', $pair);";

/// Row of `document_order` and of `document_unkeyed`.
#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
pub struct OrderRow {
    workspace_id: String,
    kind: String,
    doc_id: String,
    version: i64,
    inv: i64,
}

/// Row of `document_key`.
#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
struct KeyRow {
    workspace_id: String,
    kind: String,
    key: String,
    doc_id: String,
    version: i64,
    inv: i64,
}

/// A `document_key` row with its record key.
#[derive(Serialize, SurrealValue)]
struct KeyedKeyRow {
    key: String,
    row: KeyRow,
}

/// A place in an order, as a page reads it.
#[derive(Debug, Clone, Deserialize, SurrealValue)]
struct Place {
    inv: i64,
    doc_id: String,
    version: i64,
}

/// The record key of document `id` of `ws` in the order and the unkeyed.
pub fn pair(ws: &str, id: &str) -> String {
    format!("{ws}_{id}")
}

fn key_record(ws: &str, id: &str, key: &DocumentKey) -> String {
    format!("{ws}_{id}_{key}")
}

/// The binds of [`REINDEX`] for `latest` with `keys` (none: unkeyed).
pub struct Reindex {
    pair: String,
    order: OrderRow,
    key_rows: Vec<KeyedKeyRow>,
    unkeyed_rows: Vec<OrderRow>,
}

impl Reindex {
    /// The binds for `latest`.
    pub fn of(latest: &Document, keys: Option<&[DocumentKey]>) -> Result<Self> {
        let (ws, id) = (latest.workspace_id.to_string(), latest.id.to_string());
        let (kind, version) = (latest.kind.to_string(), i64::try_from(latest.version)?);
        let inv = inverted(latest.updated_at, &id)?;
        let key_rows = keys
            .unwrap_or_default()
            .iter()
            .map(|key| KeyedKeyRow {
                key: key_record(&ws, &id, key),
                row: KeyRow {
                    workspace_id: ws.clone(),
                    kind: kind.clone(),
                    key: key.to_string(),
                    doc_id: id.clone(),
                    version,
                    inv,
                },
            })
            .collect();
        let order = OrderRow {
            workspace_id: ws.clone(),
            kind,
            doc_id: id.clone(),
            version,
            inv,
        };
        let unkeyed_rows = match keys {
            Some(_) => Vec::new(),
            None => vec![order.clone()],
        };
        Ok(Self {
            pair: pair(&ws, &id),
            order,
            key_rows,
            unkeyed_rows,
        })
    }

    /// `query` with these binds and `$workspace_id`, `$doc_id`.
    pub fn bind<'q>(
        self,
        query: surrealdb::method::Query<'q, surrealdb::engine::any::Any>,
    ) -> surrealdb::method::Query<'q, surrealdb::engine::any::Any> {
        query
            .bind(("workspace_id", self.order.workspace_id.clone()))
            .bind(("doc_id", self.order.doc_id.clone()))
            .bind(("pair", self.pair))
            .bind(("order", self.order))
            .bind(("key_rows", self.key_rows))
            .bind(("unkeyed_rows", self.unkeyed_rows))
    }
}

/// A matching place: inverted time, id, version.
pub type Matched = (i64, Uuid, u64);

impl SurrealDBStorage {
    /// Every place `query` matches, in order, whatever its cursor: the
    /// first key's (or the kind's) that have every other key, and with keys
    /// the kind's unkeyed ones.
    pub async fn matching(&self, query: &LatestQuery<'_>) -> Result<Vec<Matched>> {
        let ws = query.workspace_id.to_string();
        let kind = query.kind.to_string();
        let (statement, first) = match query.keys.first() {
            Some(key) => (
                "SELECT inv, doc_id, version FROM document_key WHERE workspace_id = $workspace_id \
                 AND kind = $kind AND key = $key",
                key.to_string(),
            ),
            None => (
                "SELECT inv, doc_id, version FROM document_order \
                 WHERE workspace_id = $workspace_id AND kind = $kind",
                String::new(),
            ),
        };
        let mut response = self
            .db()
            .query(statement)
            .bind(("workspace_id", ws.clone()))
            .bind(("kind", kind))
            .bind(("key", first))
            .await?
            .check()?;
        let mut places: Vec<Place> = response.take(0)?;
        for other in query.keys.iter().skip(1) {
            let records: Vec<RecordId> = places
                .iter()
                .map(|p| RecordId::new("document_key", key_record(&ws, &p.doc_id, other)))
                .collect();
            let mut response = self
                .db()
                .query("SELECT VALUE doc_id FROM $records")
                .bind(("records", records))
                .await?
                .check()?;
            let present: HashSet<String> = response.take::<Vec<String>>(0)?.into_iter().collect();
            places.retain(|p| present.contains(&p.doc_id));
        }
        if !query.keys.is_empty() {
            let mut response = self
                .db()
                .query(
                    "SELECT inv, doc_id, version FROM document_unkeyed \
                     WHERE workspace_id = $workspace_id AND kind = $kind",
                )
                .bind(("workspace_id", ws.clone()))
                .bind(("kind", query.kind.to_string()))
                .await?
                .check()?;
            places.extend(response.take::<Vec<Place>>(0)?);
        }
        let mut matched = places
            .into_iter()
            .map(|p| {
                let id =
                    post_cortex_ports::error::parse_name("document order", &p.doc_id, &p.doc_id)?;
                Ok((p.inv, id, u64::try_from(p.version)?))
            })
            .collect::<Result<Vec<Matched>>>()?;
        matched.sort_unstable_by_key(|(inv, id, _)| (*inv, *id));
        Ok(matched)
    }

    /// The ids of the unkeyed latest versions of `ws`.
    pub async fn unkeyed_ids(&self, ws: Uuid) -> Result<Vec<Uuid>> {
        let mut response = self
            .db()
            .query("SELECT VALUE doc_id FROM document_unkeyed WHERE workspace_id = $workspace_id")
            .bind(("workspace_id", ws.to_string()))
            .await?
            .check()?;
        let ids: Vec<String> = response.take(0)?;
        ids.iter()
            .map(|id| {
                Ok(post_cortex_ports::error::parse_name(
                    "document unkeyed",
                    id,
                    id,
                )?)
            })
            .collect()
    }
}
