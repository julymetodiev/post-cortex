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

//! Freshness tracking and cascade invalidation for RocksDB, following the
//! state machine of [`crate::freshness`].
//!
//! Key families:
//! - `source_ref:{entry_id}` -> the reference with its state, a
//!   [`SourceRow`] (see [`crate::rocksdb_storage::source_rows`]).
//! - `source_by_file:{file_path}|{entry_id}` -> the entry id; per-file
//!   index over `source_ref:`, maintained in the same batch. The id is
//!   read from the value: a path containing `|` makes the key ambiguous.
//! - `symbol_dep:{from}|{to}` (forward) and `symbol_rdep:{to}|{from}`
//!   (reverse, used for BFS-based cascade walks).

use crate::port_error::PortResult;
use crate::port_error::Result;
use async_trait::async_trait;
use rocksdb::{DB, WriteBatch};
use std::collections::{BTreeSet, HashSet};
use uuid::Uuid;

use crate::freshness::{SourceStatus, evaluate, in_scope};
use post_cortex_core::core::freshness::{
    CascadeInvalidateReport, FreshnessEntry, SourceReference, SymbolId,
};
use post_cortex_ports::storage::{FreshnessStore, InvalidationScope, StaleEntryInfo};

use crate::rocksdb_storage::RealRocksDBStorage;
use crate::rocksdb_storage::core::{blocking, scan_prefix};
use crate::rocksdb_storage::source_rows::SourceRow;

pub const SOURCE_REF_PREFIX: &str = "source_ref:";

pub fn source_ref_key(entry_id: &str) -> String {
    format!("{SOURCE_REF_PREFIX}{entry_id}")
}

fn file_index_prefix(file_path: &str) -> String {
    format!("source_by_file:{}|", file_path)
}

fn file_index_key(file_path: &str, entry_id: &str) -> String {
    format!("source_by_file:{}|{}", file_path, entry_id)
}

/// Queue the per-file index entry of a reference.
pub fn put_file_index(batch: &mut WriteBatch, file_path: &str, entry_id: &str) {
    batch.put(
        file_index_key(file_path, entry_id).as_bytes(),
        entry_id.as_bytes(),
    );
}

/// Queue writing `source` as stale.
fn mark_stale(mut source: SourceRow, batch: &mut WriteBatch) -> Result<()> {
    source.status = SourceStatus::Stale.code();
    batch.put(
        source_ref_key(&source.entry_id).as_bytes(),
        source.encode()?,
    );
    Ok(())
}

fn load_source(db: &DB, entry_id: &str) -> Result<Option<SourceRow>> {
    let key = source_ref_key(entry_id);
    db.get(key.as_bytes())?
        .map(|row| SourceRow::decode(key.as_bytes(), &row))
        .transpose()
}

/// Every reference registered for `file_path`, found via the per-file
/// index. References are re-checked against `file_path` because a path
/// containing `|` can share an index prefix with another path. An indexed
/// reference that is missing or unreadable is an error.
fn sources_for_file(db: &DB, file_path: &str) -> Result<Vec<SourceRow>> {
    let prefix = file_index_prefix(file_path);
    let mut ref_keys = Vec::new();
    for item in scan_prefix(db, prefix.as_bytes()) {
        let (_, entry_id) = item?;
        ref_keys.push(source_ref_key(&String::from_utf8_lossy(&entry_id)));
    }

    let mut sources = Vec::with_capacity(ref_keys.len());
    for (key, value) in ref_keys.iter().zip(db.multi_get(&ref_keys)) {
        let row = value?.ok_or_else(|| {
            post_cortex_ports::error::corrupt("source reference", key, "indexed but missing")
        })?;
        let source = SourceRow::decode(key.as_bytes(), &row)?;
        if source.file_path == file_path {
            sources.push(source);
        }
    }
    Ok(sources)
}

#[async_trait]
impl FreshnessStore for RealRocksDBStorage {
    async fn register_source(
        &self,
        session_id: Uuid,
        reference: SourceReference,
    ) -> PortResult<()> {
        self.breaker
            .guard(async {
                let db = self.db.clone();
                blocking(move || {
                    let mut batch = WriteBatch::default();
                    // Re-registering under another path moves the index entry.
                    if let Some(old) = load_source(&db, &reference.entry_id)?
                        && old.file_path != reference.file_path
                    {
                        batch.delete(file_index_key(&old.file_path, &old.entry_id).as_bytes());
                    }
                    put_file_index(&mut batch, &reference.file_path, &reference.entry_id);
                    let key = source_ref_key(&reference.entry_id);
                    let source =
                        SourceRow::new(reference, SourceStatus::Registered, Some(session_id));
                    batch.put(key.as_bytes(), source.encode()?);
                    db.write(batch)?;
                    Ok(())
                })
                .await
            })
            .await
    }

    async fn check_freshness(
        &self,
        entry_id: &str,
        file_hash: &[u8],
    ) -> PortResult<FreshnessEntry> {
        self.breaker
            .guard(async {
                Ok(self
                    .check_freshness_semantic(entry_id, file_hash, None, None)
                    .await?)
            })
            .await
    }

    async fn invalidate_source(
        &self,
        scope: InvalidationScope,
        file_path: &str,
    ) -> PortResult<u32> {
        self.breaker
            .guard(async {
                let db = self.db.clone();
                let file_path = file_path.to_string();

                blocking(move || {
                    let mut batch = WriteBatch::default();
                    let mut invalidated = 0u32;
                    for source in sources_for_file(&db, &file_path)? {
                        if in_scope(scope, source.owner) {
                            mark_stale(source, &mut batch)?;
                            invalidated += 1;
                        }
                    }
                    db.write(batch)?;
                    Ok(invalidated)
                })
                .await
            })
            .await
    }

    async fn get_entries_by_source(&self, file_path: &str) -> PortResult<Vec<SourceReference>> {
        self.breaker
            .guard(async {
                let db = self.db.clone();
                let file_path = file_path.to_string();
                blocking(move || {
                    Ok(sources_for_file(&db, &file_path)?
                        .iter()
                        .map(SourceRow::reference)
                        .collect())
                })
                .await
            })
            .await
    }

    async fn get_stale_entries_by_source(
        &self,
        file_path: &str,
    ) -> PortResult<Vec<StaleEntryInfo>> {
        self.breaker
            .guard(async {
                let db = self.db.clone();
                let file_path = file_path.to_string();
                blocking(move || {
                    let mut stale = Vec::new();
                    for source in sources_for_file(&db, &file_path)? {
                        if source.status()? != SourceStatus::Stale {
                            continue;
                        }
                        let function = source.function();
                        stale.push(StaleEntryInfo {
                            symbol_name: function.map(|(name, _)| name.to_string()),
                            symbol_type: function
                                .map(|(_, symbol_type)| symbol_type.to_string())
                                .filter(|t| !t.is_empty()),
                            entry_id: source.entry_id,
                        });
                    }
                    Ok(stale)
                })
                .await
            })
            .await
    }

    async fn check_freshness_semantic(
        &self,
        entry_id: &str,
        file_hash: &[u8],
        ast_hash: Option<&[u8]>,
        _symbol_name: Option<&str>,
    ) -> PortResult<FreshnessEntry> {
        self.breaker
            .guard(async {
                let db = self.db.clone();
                let entry_id = entry_id.to_string();
                let file_hash = file_hash.to_vec();
                let ast_hash = ast_hash.map(<[u8]>::to_vec);

                blocking(move || {
                    let source = load_source(&db, &entry_id)?;
                    let state = source.as_ref().map(SourceRow::state).transpose()?;
                    Ok(evaluate(entry_id, state, file_hash, ast_hash.as_deref()))
                })
                .await
            })
            .await
    }

    /// One `multi_get` for all entries.
    async fn check_freshness_batch(
        &self,
        entries: Vec<(String, Vec<u8>, Option<Vec<u8>>, Option<String>)>,
    ) -> PortResult<Vec<FreshnessEntry>> {
        self.breaker
            .guard(async {
                let db = self.db.clone();

                blocking(move || {
                    let keys: Vec<String> =
                        entries.iter().map(|(id, ..)| source_ref_key(id)).collect();
                    let values = db.multi_get(&keys);
                    let mut results = Vec::with_capacity(entries.len());
                    for ((key, (entry_id, file_hash, ast_hash, _)), value) in
                        keys.iter().zip(entries).zip(values)
                    {
                        let source = value?
                            .map(|row| SourceRow::decode(key.as_bytes(), &row))
                            .transpose()?;
                        let state = source.as_ref().map(SourceRow::state).transpose()?;
                        results.push(evaluate(entry_id, state, file_hash, ast_hash.as_deref()));
                    }
                    Ok(results)
                })
                .await
            })
            .await
    }

    async fn register_symbol_dependencies(
        &self,
        from: SymbolId,
        to_symbols: Vec<SymbolId>,
    ) -> PortResult<u32> {
        self.breaker
            .guard(async {
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
                            let name_only_rev =
                                format!("symbol_rdep:::{}|{}", to.symbol_name, from_key);
                            batch.put(name_only_rev.as_bytes(), from.symbol_type.as_bytes());
                        }
                    }

                    db.write(batch)?;
                    Ok(to_symbols.len() as u32)
                })
                .await
            })
            .await
    }

    async fn cascade_invalidate(
        &self,
        changed: SymbolId,
        _new_ast_hash: Vec<u8>,
        max_depth: u32,
    ) -> PortResult<CascadeInvalidateReport> {
        self.breaker
            .guard(async {
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
                                let dep_key =
                                    String::from_utf8_lossy(&key[prefix.len()..]).into_owned();
                                if visited.insert(dep_key.clone()) {
                                    queue.push_back((dep_key, depth + 1));
                                }
                            }
                        }
                    }

                    // Invalidate the function-scoped references of the changed symbol
                    // (direct) and of its dependents (cascade). A symbol key is
                    // `{file}::{name}`, so only those files' references can match.
                    let dependents: HashSet<&str> =
                        dependent_symbols.iter().map(String::as_str).collect();
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
                        for source in sources_for_file(&db, file)? {
                            let Some((name, _)) = source.function() else {
                                continue;
                            };
                            let ref_key = format!("{}::{}", source.file_path, name);
                            if ref_key == changed_key {
                                direct_count += 1;
                            } else if dependents.contains(ref_key.as_str()) {
                                cascade_count += 1;
                            } else {
                                continue;
                            }
                            mark_stale(source, &mut batch)?;
                        }
                    }
                    db.write(batch)?;

                    Ok(CascadeInvalidateReport {
                        direct_invalidations: direct_count,
                        cascade_invalidations: cascade_count,
                        invalidated_symbols: dependent_symbols,
                    })
                })
                .await
            })
            .await
    }
}

#[cfg(test)]
mod tests;
