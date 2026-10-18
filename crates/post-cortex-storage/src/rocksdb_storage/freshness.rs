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

//! Freshness tracking and cascade invalidation for RocksDB.
//!
//! Key families:
//! - `source_ref:{entry_id}` -> protobuf [`SourceReference`]
//! - `source_by_file:{file_path}|{entry_id}` -> empty; per-file index over
//!   `source_ref:`, maintained in the same batch. Databases created before
//!   it existed are indexed once on open (marker [`SOURCE_INDEX_MARKER`]).
//! - `symbol_dep:{from}|{to}` (forward) and `symbol_rdep:{to}|{from}`
//!   (reverse, used for BFS-based cascade walks).

use anyhow::Result;
use async_trait::async_trait;
use prost::Message;
use rocksdb::{DB, WriteBatch};
use std::collections::{BTreeSet, HashSet};
use tracing::info;

use crate::traits::FreshnessStorage;
use post_cortex_proto::pb::{FreshnessEntry, FreshnessStatus, SourceReference};

use super::RealRocksDBStorage;
use super::core::{blocking, scan_prefix};

const SOURCE_REF_PREFIX: &str = "source_ref:";
/// Present once every `source_ref:` record has its `source_by_file:` entry.
const SOURCE_INDEX_MARKER: &[u8] = b"index_version:source_by_file";

fn source_ref_key(entry_id: &str) -> String {
    format!("source_ref:{}", entry_id)
}

fn file_index_prefix(file_path: &str) -> String {
    format!("source_by_file:{}|", file_path)
}

fn file_index_key(file_path: &str, entry_id: &str) -> String {
    format!("source_by_file:{}|{}", file_path, entry_id)
}

/// Name of the function scope of `reference`, if it has one.
fn function_name(reference: &SourceReference) -> Option<&str> {
    use post_cortex_proto::pb::source_scope::Scope;
    match reference.scope.as_ref()?.scope.as_ref()? {
        Scope::Function(func) => Some(func.name.as_str()),
        _ => None,
    }
}

/// Compare a stored reference against the client's hashes. The AST hash is
/// used when both sides have one for a function scope (semantic freshness),
/// the file hash otherwise.
fn freshness_of(
    entry_id: String,
    reference: Option<SourceReference>,
    file_hash: Vec<u8>,
    ast_hash: Option<&[u8]>,
) -> FreshnessEntry {
    let Some(reference) = reference else {
        // Entry has no source tracking
        return FreshnessEntry {
            entry_id,
            file_path: String::new(),
            status: FreshnessStatus::Unknown as i32,
            stored_hash: Vec::new(),
            current_hash: file_hash,
        };
    };

    use post_cortex_proto::pb::source_scope::Scope;
    let stored_ast = match reference.scope.as_ref().and_then(|s| s.scope.as_ref()) {
        Some(Scope::Function(func)) if !func.ast_hash.is_empty() => Some(&func.ast_hash),
        _ => None,
    };
    let is_fresh = match (ast_hash, stored_ast) {
        (Some(client), Some(stored)) => stored.as_slice() == client,
        _ => reference.content_hash == file_hash,
    };

    FreshnessEntry {
        entry_id,
        file_path: reference.file_path,
        status: if is_fresh {
            FreshnessStatus::Fresh as i32
        } else {
            FreshnessStatus::Stale as i32
        },
        stored_hash: reference.content_hash,
        current_hash: file_hash,
    }
}

fn decode_reference(bytes: &[u8]) -> Result<SourceReference> {
    SourceReference::decode(bytes)
        .map_err(|e| anyhow::anyhow!("Failed to deserialize source reference: {}", e))
}

fn load_reference(db: &DB, entry_id: &str) -> Result<Option<SourceReference>> {
    db.get(source_ref_key(entry_id).as_bytes())?
        .map(|data| decode_reference(&data))
        .transpose()
}

/// Every reference registered for `file_path`, found via the per-file
/// index. References are re-checked against `file_path` because a path
/// containing `|` can share an index prefix with another path.
fn references_for_file(db: &DB, file_path: &str) -> Result<Vec<SourceReference>> {
    let prefix = file_index_prefix(file_path);
    let mut ref_keys = Vec::new();
    for item in scan_prefix(db, prefix.as_bytes()) {
        let (key, _) = item?;
        let entry_id = String::from_utf8_lossy(&key[prefix.len()..]);
        ref_keys.push(source_ref_key(&entry_id));
    }

    let mut references = Vec::with_capacity(ref_keys.len());
    for value in db.multi_get(&ref_keys) {
        if let Some(data) = value?
            && let Ok(reference) = SourceReference::decode(&data[..])
            && reference.file_path == file_path
        {
            references.push(reference);
        }
    }
    Ok(references)
}

/// Queue deletion of a reference and its index entry.
fn delete_reference(batch: &mut WriteBatch, reference: &SourceReference) {
    batch.delete(source_ref_key(&reference.entry_id).as_bytes());
    batch.delete(file_index_key(&reference.file_path, &reference.entry_id).as_bytes());
}

/// Index every `source_ref:` record by file, once per database.
fn ensure_file_index(db: &DB) -> Result<()> {
    if db.get(SOURCE_INDEX_MARKER)?.is_some() {
        return Ok(());
    }
    let mut batch = WriteBatch::default();
    let mut indexed = 0usize;
    for item in scan_prefix(db, SOURCE_REF_PREFIX.as_bytes()) {
        let (_, value) = item?;
        if let Ok(reference) = SourceReference::decode(&value[..]) {
            batch.put(
                file_index_key(&reference.file_path, &reference.entry_id).as_bytes(),
                [],
            );
            indexed += 1;
        }
    }
    batch.put(SOURCE_INDEX_MARKER, b"1");
    db.write(batch)?;
    if indexed > 0 {
        info!(
            "RealRocksDBStorage: Indexed {} source references by file",
            indexed
        );
    }
    Ok(())
}

impl RealRocksDBStorage {
    /// Build the `source_by_file:` index for databases written before it
    /// existed. Cheap no-op once the marker is present.
    pub(super) async fn ensure_source_file_index(&self) -> Result<()> {
        let db = self.db.clone();
        blocking(move || ensure_file_index(&db)).await
    }
}

#[async_trait]
impl FreshnessStorage for RealRocksDBStorage {
    async fn register_source(
        &self,
        _session_id: uuid::Uuid,
        reference: SourceReference,
    ) -> Result<()> {
        let db = self.db.clone();
        blocking(move || {
            let mut batch = WriteBatch::default();
            // Re-registering under another path moves the index entry.
            if let Some(old) = load_reference(&db, &reference.entry_id)?
                && old.file_path != reference.file_path
            {
                batch.delete(file_index_key(&old.file_path, &old.entry_id).as_bytes());
            }
            batch.put(
                source_ref_key(&reference.entry_id).as_bytes(),
                reference.encode_to_vec(),
            );
            batch.put(
                file_index_key(&reference.file_path, &reference.entry_id).as_bytes(),
                [],
            );
            db.write(batch)?;
            Ok(())
        })
        .await
    }

    async fn check_freshness(&self, entry_id: &str, file_hash: &[u8]) -> Result<FreshnessEntry> {
        self.check_freshness_semantic(entry_id, file_hash, None, None)
            .await
    }

    async fn invalidate_source(&self, file_path: &str) -> Result<u32> {
        let db = self.db.clone();
        let file_path = file_path.to_string();

        blocking(move || {
            let references = references_for_file(&db, &file_path)?;
            let mut batch = WriteBatch::default();
            for reference in &references {
                delete_reference(&mut batch, reference);
            }
            db.write(batch)?;
            Ok(references.len() as u32)
        })
        .await
    }

    async fn get_entries_by_source(&self, file_path: &str) -> Result<Vec<SourceReference>> {
        let db = self.db.clone();
        let file_path = file_path.to_string();
        blocking(move || references_for_file(&db, &file_path)).await
    }

    async fn get_stale_entries_by_source(
        &self,
        _file_path: &str,
    ) -> Result<Vec<crate::traits::StaleEntryInfo>> {
        // RocksDB does not track per-record stale status; it DELETEs on invalidation.
        // Return empty — deleted-symbol detection is only supported on SurrealDB.
        Ok(Vec::new())
    }

    async fn check_freshness_semantic(
        &self,
        entry_id: &str,
        file_hash: &[u8],
        ast_hash: Option<&[u8]>,
        _symbol_name: Option<&str>,
    ) -> Result<FreshnessEntry> {
        let db = self.db.clone();
        let entry_id = entry_id.to_string();
        let file_hash = file_hash.to_vec();
        let ast_hash = ast_hash.map(<[u8]>::to_vec);

        blocking(move || {
            let reference = load_reference(&db, &entry_id)?;
            Ok(freshness_of(
                entry_id,
                reference,
                file_hash,
                ast_hash.as_deref(),
            ))
        })
        .await
    }

    /// One `multi_get` for all entries. Undecodable records report
    /// `Unknown`, like the per-entry default implementation.
    async fn check_freshness_batch(
        &self,
        entries: Vec<(String, Vec<u8>, Option<Vec<u8>>, Option<String>)>,
    ) -> Result<Vec<FreshnessEntry>> {
        let db = self.db.clone();

        blocking(move || {
            let keys: Vec<String> = entries.iter().map(|(id, ..)| source_ref_key(id)).collect();
            let values = db.multi_get(&keys);
            let mut results = Vec::with_capacity(entries.len());
            for ((entry_id, file_hash, ast_hash, _), value) in entries.into_iter().zip(values) {
                let reference = value
                    .ok()
                    .flatten()
                    .and_then(|data| SourceReference::decode(&data[..]).ok());
                results.push(freshness_of(
                    entry_id,
                    reference,
                    file_hash,
                    ast_hash.as_deref(),
                ));
            }
            Ok(results)
        })
        .await
    }

    async fn register_symbol_dependencies(
        &self,
        from: post_cortex_proto::pb::SymbolId,
        to_symbols: Vec<post_cortex_proto::pb::SymbolId>,
    ) -> Result<u32> {
        let db = self.db.clone();

        blocking(move || {
            let mut batch = WriteBatch::default();
            let from_key = format!("{}::{}", from.file_path, from.symbol_name);

            for to in &to_symbols {
                let to_key = if to.file_path.is_empty() {
                    // Name-only dependency (file unknown)
                    format!("::{}", to.symbol_name)
                } else {
                    format!("{}::{}", to.file_path, to.symbol_name)
                };
                // Forward edge: symbol_dep:{from} -> {to}
                let fwd = format!("symbol_dep:{}|{}", from_key, to_key);
                batch.put(fwd.as_bytes(), to.symbol_type.as_bytes());
                // Reverse edge: symbol_rdep:{to} -> {from} (for cascade lookup)
                let rev = format!("symbol_rdep:{}|{}", to_key, from_key);
                batch.put(rev.as_bytes(), from.symbol_type.as_bytes());

                // Also store name-only reverse edge if file is known,
                // so cascade can match even when dependency was registered without file
                if !to.file_path.is_empty() {
                    let name_only_rev = format!("symbol_rdep:::{}|{}", to.symbol_name, from_key);
                    batch.put(name_only_rev.as_bytes(), from.symbol_type.as_bytes());
                }
            }

            db.write(batch)?;
            Ok(to_symbols.len() as u32)
        })
        .await
    }

    async fn cascade_invalidate(
        &self,
        changed: post_cortex_proto::pb::SymbolId,
        _new_ast_hash: Vec<u8>,
        max_depth: u32,
    ) -> Result<post_cortex_proto::pb::CascadeInvalidateReport> {
        let db = self.db.clone();

        blocking(move || {
            use std::collections::VecDeque;

            let changed_key = format!("{}::{}", changed.file_path, changed.symbol_name);

            // BFS: find all symbols that depend on the changed symbol (reverse edges)
            let mut visited = HashSet::new();
            let mut queue = VecDeque::new();
            queue.push_back((changed_key.clone(), 0u32));
            visited.insert(changed_key.clone());

            let mut dependent_symbols = Vec::new();

            while let Some((sym_key, depth)) = queue.pop_front() {
                if depth > 0 {
                    dependent_symbols.push(sym_key.clone());
                }
                if depth >= max_depth {
                    continue;
                }

                // Find reverse deps: who depends on sym_key?
                // Search both full key (file::symbol) and name-only (::symbol)
                let sym_name = sym_key.split("::").last().unwrap_or(&sym_key);
                let prefixes = [
                    format!("symbol_rdep:{}|", sym_key),
                    format!("symbol_rdep:::{}|", sym_name),
                ];

                for prefix in &prefixes {
                    for item in scan_prefix(&db, prefix.as_bytes()) {
                        let (key, _) = item?;
                        let dep_key = String::from_utf8_lossy(&key[prefix.len()..]).into_owned();
                        if visited.insert(dep_key.clone()) {
                            queue.push_back((dep_key, depth + 1));
                        }
                    }
                }
            }

            // Invalidate the function-scoped references of the changed symbol
            // (direct) and of its dependents (cascade). A symbol key is
            // `{file}::{name}`, so only those files' references can match.
            let dependents: HashSet<&str> = dependent_symbols.iter().map(String::as_str).collect();
            let files: BTreeSet<&str> = std::iter::once(changed.file_path.as_str())
                .chain(
                    dependent_symbols
                        .iter()
                        .filter_map(|key| key.split_once("::").map(|(file, _)| file)),
                )
                .collect();

            let mut direct_count = 0u32;
            let mut cascade_count = 0u32;
            let mut batch = WriteBatch::default();

            for file in files {
                for reference in references_for_file(&db, file)? {
                    let Some(name) = function_name(&reference) else {
                        continue;
                    };
                    let ref_key = format!("{}::{}", reference.file_path, name);
                    if ref_key == changed_key {
                        direct_count += 1;
                    } else if dependents.contains(ref_key.as_str()) {
                        cascade_count += 1;
                    } else {
                        continue;
                    }
                    delete_reference(&mut batch, &reference);
                }
            }
            db.write(batch)?;

            Ok(post_cortex_proto::pb::CascadeInvalidateReport {
                direct_invalidations: direct_count,
                cascade_invalidations: cascade_count,
                invalidated_symbols: dependent_symbols,
            })
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use post_cortex_proto::pb::{FunctionScope, SourceScope, SymbolId, source_scope};
    use tempfile::tempdir;

    fn reference(entry_id: &str, file: &str, func: Option<&str>) -> SourceReference {
        SourceReference {
            entry_id: entry_id.to_string(),
            file_path: file.to_string(),
            content_hash: b"file-hash".to_vec(),
            captured_at_unix: 0,
            scope: func.map(|name| SourceScope {
                scope: Some(source_scope::Scope::Function(FunctionScope {
                    name: name.to_string(),
                    ast_hash: format!("ast-{name}").into_bytes(),
                    symbol_type: "fn".to_string(),
                    imports: vec![],
                })),
            }),
        }
    }

    fn symbol(file: &str, name: &str) -> SymbolId {
        SymbolId {
            file_path: file.to_string(),
            symbol_name: name.to_string(),
            symbol_type: "fn".to_string(),
        }
    }

    #[tokio::test]
    async fn file_index_drives_lookup_and_invalidation() {
        let dir = tempdir().unwrap();
        let storage = RealRocksDBStorage::new(dir.path()).await.unwrap();
        let sid = uuid::Uuid::new_v4();

        storage
            .register_source(sid, reference("e1", "a.rs", None))
            .await
            .unwrap();
        storage
            .register_source(sid, reference("e2", "a.rs", None))
            .await
            .unwrap();
        storage
            .register_source(sid, reference("e3", "a.rs|b", None))
            .await
            .unwrap();
        storage
            .register_source(sid, reference("e4", "b.rs", None))
            .await
            .unwrap();
        // e4 moves to a.rs: its old index entry must not linger.
        storage
            .register_source(sid, reference("e4", "a.rs", None))
            .await
            .unwrap();

        let mut ids: Vec<String> = storage
            .get_entries_by_source("a.rs")
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.entry_id)
            .collect();
        ids.sort();
        assert_eq!(ids, vec!["e1", "e2", "e4"]);
        assert!(
            storage
                .get_entries_by_source("b.rs")
                .await
                .unwrap()
                .is_empty()
        );

        assert_eq!(storage.invalidate_source("a.rs").await.unwrap(), 3);
        assert!(
            storage
                .get_entries_by_source("a.rs")
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            storage.get_entries_by_source("a.rs|b").await.unwrap().len(),
            1
        );
        let unknown = storage.check_freshness("e1", b"file-hash").await.unwrap();
        assert_eq!(unknown.status, FreshnessStatus::Unknown as i32);
    }

    #[tokio::test]
    async fn references_from_before_the_index_are_indexed_on_open() {
        let dir = tempdir().unwrap();
        {
            let storage = RealRocksDBStorage::new(dir.path()).await.unwrap();
            // Simulate an old database: raw record, no index, no marker.
            storage
                .db
                .put(
                    b"source_ref:old",
                    reference("old", "legacy.rs", None).encode_to_vec(),
                )
                .unwrap();
            storage.db.delete(SOURCE_INDEX_MARKER).unwrap();
        }
        let storage = RealRocksDBStorage::new(dir.path()).await.unwrap();
        let found = storage.get_entries_by_source("legacy.rs").await.unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(storage.invalidate_source("legacy.rs").await.unwrap(), 1);
    }

    #[tokio::test]
    async fn batch_check_matches_single_checks() {
        let dir = tempdir().unwrap();
        let storage = RealRocksDBStorage::new(dir.path()).await.unwrap();
        let sid = uuid::Uuid::new_v4();
        storage
            .register_source(sid, reference("fn1", "a.rs", Some("foo")))
            .await
            .unwrap();
        storage
            .register_source(sid, reference("file1", "a.rs", None))
            .await
            .unwrap();

        let entries = vec![
            // AST hash matches although the file changed -> fresh
            (
                "fn1".to_string(),
                b"changed".to_vec(),
                Some(b"ast-foo".to_vec()),
                None,
            ),
            // AST hash differs -> stale
            (
                "fn1".to_string(),
                b"file-hash".to_vec(),
                Some(b"other".to_vec()),
                None,
            ),
            // File-level reference, hash unchanged -> fresh
            ("file1".to_string(), b"file-hash".to_vec(), None, None),
            ("missing".to_string(), b"x".to_vec(), None, None),
        ];
        let mut single = Vec::new();
        for (id, file_hash, ast, name) in &entries {
            single.push(
                storage
                    .check_freshness_semantic(id, file_hash, ast.as_deref(), name.as_deref())
                    .await
                    .unwrap(),
            );
        }
        let batch = storage.check_freshness_batch(entries).await.unwrap();
        assert_eq!(batch, single);
        let statuses: Vec<i32> = batch.iter().map(|e| e.status).collect();
        assert_eq!(
            statuses,
            vec![
                FreshnessStatus::Fresh as i32,
                FreshnessStatus::Stale as i32,
                FreshnessStatus::Fresh as i32,
                FreshnessStatus::Unknown as i32,
            ]
        );
    }

    #[tokio::test]
    async fn cascade_invalidates_direct_and_dependent_symbols() {
        let dir = tempdir().unwrap();
        let storage = RealRocksDBStorage::new(dir.path()).await.unwrap();
        let sid = uuid::Uuid::new_v4();
        storage
            .register_source(sid, reference("core", "lib.rs", Some("core")))
            .await
            .unwrap();
        storage
            .register_source(sid, reference("user", "app.rs", Some("user")))
            .await
            .unwrap();
        storage
            .register_source(sid, reference("other", "app.rs", Some("other")))
            .await
            .unwrap();

        // app.rs::user depends on lib.rs::core
        storage
            .register_symbol_dependencies(symbol("app.rs", "user"), vec![symbol("lib.rs", "core")])
            .await
            .unwrap();

        let report = storage
            .cascade_invalidate(symbol("lib.rs", "core"), vec![], 3)
            .await
            .unwrap();
        assert_eq!(report.direct_invalidations, 1);
        assert_eq!(report.cascade_invalidations, 1);
        assert_eq!(report.invalidated_symbols, vec!["app.rs::user"]);

        let remaining: Vec<String> = storage
            .get_entries_by_source("app.rs")
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.entry_id)
            .collect();
        assert_eq!(remaining, vec!["other"]);
    }
}
