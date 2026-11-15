// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The order and the keys of the latest versions of documents
//! (docs/memory.md, document keys and pages). Key families (see
//! [`crate::rocksdb_storage::rows`]), `{inv}` the time of the latest
//! version inverted ([`crate::document_order`]) in 19 digits:
//! - `doc_order:{ws}:{kind}:{inv}:{id}` -> empty: the kind's documents,
//!   the most recently changed first.
//! - `doc_key:{ws}:{kind}:{key}:{inv}:{id}` -> empty: those with a key.
//! - `doc_keys:{ws}:{id}` -> the latest's keys, marked JSON, to take them
//!   out when it changes.
//! - `doc_unkeyed:{ws}:{kind}:{inv}:{id}` -> empty: the latest has no keys;
//!   a page with keys holds them too, for memory to check.

use std::str::FromStr;

use rocksdb::{DB, WriteBatch};
use uuid::Uuid;

use post_cortex_core::core::document::{Document, DocumentKey, DocumentKind};
use post_cortex_ports::error::corrupt;

use crate::document_order::inverted;
use crate::port_error::Result;
use crate::rocksdb_storage::core::scan_prefix;
use crate::rocksdb_storage::rows::{DOCUMENT_KEYS_V1, decode_json_marked, json_marked};
use post_cortex_ports::storage::LatestQuery;

pub fn order_prefix(ws: Uuid, kind: &DocumentKind) -> String {
    format!("doc_order:{ws}:{kind}:")
}

fn key_prefix(ws: Uuid, kind: &DocumentKind, key: &DocumentKey) -> String {
    format!("doc_key:{ws}:{kind}:{key}:")
}

fn keys_key(ws: Uuid, id: Uuid) -> String {
    format!("doc_keys:{ws}:{id}")
}

pub fn unkeyed_prefix(ws: Uuid) -> String {
    format!("doc_unkeyed:{ws}:")
}

fn unkeyed_kind_prefix(ws: Uuid, kind: &DocumentKind) -> String {
    format!("doc_unkeyed:{ws}:{kind}:")
}

/// The prefixes of every key family of `ws` here.
pub fn prefixes(ws: Uuid) -> [String; 4] {
    [
        format!("doc_order:{ws}:"),
        format!("doc_key:{ws}:"),
        format!("doc_keys:{ws}:"),
        unkeyed_prefix(ws),
    ]
}

fn place_of(document: &Document) -> Result<String> {
    let inv = inverted(document.updated_at, &document.id.to_string())?;
    Ok(format!("{inv:019}:{}", document.id))
}

/// The place at the end of `key` after `prefix`: its inverted time and id.
fn place_after(prefix: &str, key: &[u8]) -> Result<(i64, Uuid)> {
    let text = String::from_utf8_lossy(key);
    let place = text
        .strip_prefix(prefix)
        .and_then(|rest| rest.split_once(':'))
        .and_then(|(inv, id)| Some((inv.parse().ok()?, Uuid::parse_str(id).ok()?)));
    place.ok_or_else(|| corrupt("document order", text.as_ref(), "no time and id").into())
}

fn stored_keys(db: &DB, ws: Uuid, id: Uuid) -> Result<Vec<DocumentKey>> {
    let key = keys_key(ws, id);
    let Some(row) = db.get(key.as_bytes())? else {
        return Ok(Vec::new());
    };
    let texts: Vec<String> = decode_json_marked("document keys", &key, DOCUMENT_KEYS_V1, &row)?;
    texts
        .iter()
        .map(|text| {
            DocumentKey::from_str(text).map_err(|e| corrupt("document keys", &key, e).into())
        })
        .collect()
}

/// Take `latest`, the latest version, out of its order and its keys.
pub fn unindex(db: &DB, batch: &mut WriteBatch, latest: &Document) -> Result<()> {
    let (ws, id, kind) = (latest.workspace_id, latest.id, &latest.kind);
    let place = place_of(latest)?;
    batch.delete(format!("{}{place}", order_prefix(ws, kind)).as_bytes());
    for key in stored_keys(db, ws, id)? {
        batch.delete(format!("{}{place}", key_prefix(ws, kind, &key)).as_bytes());
    }
    batch.delete(keys_key(ws, id).as_bytes());
    batch.delete(format!("{}{place}", unkeyed_kind_prefix(ws, kind)).as_bytes());
    Ok(())
}

/// Put `latest`, the latest version, in its order, with `keys` (none:
/// unkeyed).
pub fn index(
    batch: &mut WriteBatch,
    latest: &Document,
    keys: Option<&[DocumentKey]>,
) -> Result<()> {
    let (ws, id, kind) = (latest.workspace_id, latest.id, &latest.kind);
    let place = place_of(latest)?;
    batch.put(format!("{}{place}", order_prefix(ws, kind)).as_bytes(), []);
    match keys {
        Some(keys) => {
            for key in keys {
                batch.put(
                    format!("{}{place}", key_prefix(ws, kind, key)).as_bytes(),
                    [],
                );
            }
            let texts: Vec<String> = keys.iter().map(ToString::to_string).collect();
            batch.put(
                keys_key(ws, id).as_bytes(),
                json_marked(DOCUMENT_KEYS_V1, &texts)?,
            );
        }
        None => batch.put(
            format!("{}{place}", unkeyed_kind_prefix(ws, kind)).as_bytes(),
            [],
        ),
    }
    Ok(())
}

/// The ids under `prefix` (one per key), in key order.
pub fn ids_under(db: &DB, prefix: &str) -> Result<Vec<Uuid>> {
    let mut ids = Vec::new();
    for item in scan_prefix(db, prefix.as_bytes()) {
        let (key, _) = item?;
        let text = String::from_utf8_lossy(&key);
        let id = text
            .rsplit(':')
            .next()
            .and_then(|id| Uuid::parse_str(id).ok())
            .ok_or_else(|| corrupt("document index", text.as_ref(), "no document id"))?;
        ids.push(id);
    }
    Ok(ids)
}

/// Every place `query` matches, in order, whatever its cursor: the first
/// key's places (or the kind's order) that have every other key, and with
/// keys the kind's unkeyed ones.
pub fn matching(db: &DB, query: &LatestQuery<'_>) -> Result<Vec<(i64, Uuid, Uuid)>> {
    let mut matched = keyed(db, query)?;
    if !query.keys.is_empty() {
        let prefix = unkeyed_kind_prefix(query.workspace_id, query.kind);
        for item in scan_prefix(db, prefix.as_bytes()) {
            let (key, _) = item?;
            let (inv, id) = place_after(&prefix, &key)?;
            matched.push((inv, id, id));
        }
        matched.sort_unstable_by_key(|(inv, id, _)| (*inv, *id));
    }
    Ok(matched)
}

/// The places with every key of `query`, in order.
fn keyed(db: &DB, query: &LatestQuery<'_>) -> Result<Vec<(i64, Uuid, Uuid)>> {
    let (ws, kind) = (query.workspace_id, query.kind);
    let (prefix, rest) = match query.keys.split_first() {
        Some((first, rest)) => (key_prefix(ws, kind, first), rest),
        None => (order_prefix(ws, kind), &[][..]),
    };
    let mut matched = Vec::new();
    for item in scan_prefix(db, prefix.as_bytes()) {
        let (key, _) = item?;
        let (inv, id) = place_after(&prefix, &key)?;
        let place = format!("{inv:019}:{id}");
        let mut has_all = true;
        for other in rest {
            let other_key = format!("{}{place}", key_prefix(ws, kind, other));
            if db.get_pinned(other_key.as_bytes())?.is_none() {
                has_all = false;
                break;
            }
        }
        if has_all {
            matched.push((inv, id, id));
        }
    }
    Ok(matched)
}

/// Data version 4: every document already stored gets its place in its
/// kind's order and is unkeyed; the `doc_kind` rows it replaces go. Each
/// latest version is read through `latest`.
pub fn order_existing(
    db: &DB,
    latest: impl Fn(&DB, Uuid, Uuid) -> Result<Option<Document>>,
) -> Result<usize> {
    let mut placed = 0;
    let mut batch = WriteBatch::default();
    for item in scan_prefix(db, b"doc_kind:") {
        let (key, _) = item?;
        let text = String::from_utf8_lossy(&key).into_owned();
        let (Some(ws), Some(id)) = (
            text.split(':')
                .nth(1)
                .and_then(|ws| Uuid::parse_str(ws).ok()),
            text.rsplit(':')
                .next()
                .and_then(|id| Uuid::parse_str(id).ok()),
        ) else {
            return Err(corrupt("document kind row", &text, "no workspace and id").into());
        };
        batch.delete(&key);
        if let Some(document) = latest(db, ws, id)? {
            index(&mut batch, &document, None)?;
            placed += 1;
        }
        if batch.len() >= 1024 {
            db.write(std::mem::take(&mut batch))?;
        }
    }
    db.write(batch)?;
    Ok(placed)
}
