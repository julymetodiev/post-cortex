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
//! Source references and symbol dependencies live in three key families:
//! `source_ref:`, `symbol_dep:` (forward), `symbol_rdep:` (reverse, used for
//! BFS-based cascade walks).

use anyhow::Result;
use async_trait::async_trait;
use prost::Message;

use crate::traits::FreshnessStorage;
use post_cortex_proto::pb::SourceReference;

use super::RealRocksDBStorage;

#[async_trait]
impl FreshnessStorage for RealRocksDBStorage {
    async fn register_source(
        &self,
        _session_id: uuid::Uuid,
        reference: SourceReference,
    ) -> Result<()> {
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let key = format!("source_ref:{}", reference.entry_id);
            let mut data = Vec::with_capacity(reference.encoded_len());
            reference.encode(&mut data)?;
            db.put(key.as_bytes(), data)?;
            Ok(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))??;
        Ok(())
    }

    async fn check_freshness(
        &self,
        entry_id: &str,
        file_hash: &[u8],
    ) -> Result<post_cortex_proto::pb::FreshnessEntry> {
        let db = self.db.clone();
        let entry_id = entry_id.to_string();
        let current_hash = file_hash.to_vec();

        tokio::task::spawn_blocking(move || -> Result<post_cortex_proto::pb::FreshnessEntry> {
            let key = format!("source_ref:{}", entry_id);
            match db.get(key.as_bytes())? {
                Some(data) => {
                    let reference = SourceReference::decode(&data[..]).map_err(|e| {
                        anyhow::anyhow!("Failed to deserialize source reference: {}", e)
                    })?;

                    let is_fresh = reference.content_hash == current_hash;

                    let status = if is_fresh {
                        post_cortex_proto::pb::FreshnessStatus::Fresh as i32
                    } else {
                        post_cortex_proto::pb::FreshnessStatus::Stale as i32
                    };

                    Ok(post_cortex_proto::pb::FreshnessEntry {
                        entry_id,
                        file_path: reference.file_path,
                        status,
                        stored_hash: reference.content_hash,
                        current_hash,
                    })
                }
                None => {
                    // Entry has no source tracking
                    Ok(post_cortex_proto::pb::FreshnessEntry {
                        entry_id,
                        file_path: String::new(),
                        status: post_cortex_proto::pb::FreshnessStatus::Unknown as i32,
                        stored_hash: Vec::new(),
                        current_hash,
                    })
                }
            }
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))?
    }

    async fn invalidate_source(&self, file_path: &str) -> Result<u32> {
        let db = self.db.clone();
        let query_path = file_path.to_string();

        tokio::task::spawn_blocking(move || -> Result<u32> {
            let mut count = 0;
            let iter = db.iterator(rocksdb::IteratorMode::From(
                b"source_ref:",
                rocksdb::Direction::Forward,
            ));

            let mut keys_to_delete = Vec::new();

            for item in iter {
                let (key, value) = item?;
                let key_str = String::from_utf8_lossy(&key);

                if !key_str.starts_with("source_ref:") {
                    break;
                }

                if let Ok(reference) = SourceReference::decode(&value[..]) {
                    if reference.file_path == query_path {
                        keys_to_delete.push(key.to_vec());
                    }
                }
            }

            for key in keys_to_delete {
                db.delete(&key)?;
                count += 1;
            }

            Ok(count)
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))?
    }

    async fn get_entries_by_source(
        &self,
        file_path: &str,
    ) -> Result<Vec<post_cortex_proto::pb::SourceReference>> {
        let db = self.db.clone();
        let query_path = file_path.to_string();

        tokio::task::spawn_blocking(
            move || -> Result<Vec<post_cortex_proto::pb::SourceReference>> {
                let mut matches = Vec::new();
                let iter = db.iterator(rocksdb::IteratorMode::From(
                    b"source_ref:",
                    rocksdb::Direction::Forward,
                ));

                for item in iter {
                    let (key, value) = item?;
                    let key_str = String::from_utf8_lossy(&key);

                    if !key_str.starts_with("source_ref:") {
                        break;
                    }

                    if let Ok(reference) = SourceReference::decode(&value[..]) {
                        if reference.file_path == query_path {
                            matches.push(reference);
                        }
                    }
                }

                Ok(matches)
            },
        )
        .await
        .map_err(|e: tokio::task::JoinError| anyhow::anyhow!("Task join error: {}", e))?
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
    ) -> Result<post_cortex_proto::pb::FreshnessEntry> {
        let db = self.db.clone();
        let entry_id = entry_id.to_string();
        let current_file_hash = file_hash.to_vec();
        let current_ast_hash = ast_hash.map(|h| h.to_vec());

        tokio::task::spawn_blocking(move || -> Result<post_cortex_proto::pb::FreshnessEntry> {
            let key = format!("source_ref:{}", entry_id);
            match db.get(key.as_bytes())? {
                Some(data) => {
                    let reference = SourceReference::decode(&data[..]).map_err(|e| {
                        anyhow::anyhow!("Failed to deserialize source reference: {}", e)
                    })?;

                    // Semantic freshness: prefer ast_hash comparison when both sides have it
                    let is_fresh = if let (Some(client_ast), Some(scope)) =
                        (&current_ast_hash, &reference.scope)
                    {
                        use post_cortex_proto::pb::source_scope::Scope;
                        match &scope.scope {
                            Some(Scope::Function(func)) if !func.ast_hash.is_empty() => {
                                // AST-level: compare function body hashes
                                func.ast_hash == *client_ast
                            }
                            _ => reference.content_hash == current_file_hash,
                        }
                    } else {
                        // Fallback: file-level hash comparison
                        reference.content_hash == current_file_hash
                    };

                    let status = if is_fresh {
                        post_cortex_proto::pb::FreshnessStatus::Fresh as i32
                    } else {
                        post_cortex_proto::pb::FreshnessStatus::Stale as i32
                    };

                    Ok(post_cortex_proto::pb::FreshnessEntry {
                        entry_id,
                        file_path: reference.file_path,
                        status,
                        stored_hash: reference.content_hash,
                        current_hash: current_file_hash,
                    })
                }
                None => Ok(post_cortex_proto::pb::FreshnessEntry {
                    entry_id,
                    file_path: String::new(),
                    status: post_cortex_proto::pb::FreshnessStatus::Unknown as i32,
                    stored_hash: Vec::new(),
                    current_hash: current_file_hash,
                }),
            }
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))?
    }

    async fn register_symbol_dependencies(
        &self,
        from: post_cortex_proto::pb::SymbolId,
        to_symbols: Vec<post_cortex_proto::pb::SymbolId>,
    ) -> Result<u32> {
        let db = self.db.clone();

        tokio::task::spawn_blocking(move || -> Result<u32> {
            let mut count = 0u32;
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
                db.put(fwd.as_bytes(), to.symbol_type.as_bytes())?;
                // Reverse edge: symbol_rdep:{to} -> {from} (for cascade lookup)
                let rev = format!("symbol_rdep:{}|{}", to_key, from_key);
                db.put(rev.as_bytes(), from.symbol_type.as_bytes())?;

                // Also store name-only reverse edge if file is known,
                // so cascade can match even when dependency was registered without file
                if !to.file_path.is_empty() {
                    let name_only_rev = format!("symbol_rdep:::{}|{}", to.symbol_name, from_key);
                    db.put(name_only_rev.as_bytes(), from.symbol_type.as_bytes())?;
                }
                count += 1;
            }

            Ok(count)
        })
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))?
    }

    async fn cascade_invalidate(
        &self,
        changed: post_cortex_proto::pb::SymbolId,
        _new_ast_hash: Vec<u8>,
        max_depth: u32,
    ) -> Result<post_cortex_proto::pb::CascadeInvalidateReport> {
        let db = self.db.clone();

        tokio::task::spawn_blocking(
            move || -> Result<post_cortex_proto::pb::CascadeInvalidateReport> {
                use std::collections::{HashSet, VecDeque};

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
                        let iter = db.iterator(rocksdb::IteratorMode::From(
                            prefix.as_bytes(),
                            rocksdb::Direction::Forward,
                        ));

                        for item in iter {
                            let (key, _) = item?;
                            let key_str = String::from_utf8_lossy(&key);
                            if !key_str.starts_with(prefix.as_str()) {
                                break;
                            }
                            let dep_key = key_str[prefix.len()..].to_string();
                            if visited.insert(dep_key.clone()) {
                                queue.push_back((dep_key, depth + 1));
                            }
                        }
                    }
                }

                // Invalidate source_ref entries for the changed symbol's file
                let mut direct_count = 0u32;
                let mut cascade_count = 0u32;

                // Direct: invalidate entries for the changed symbol's file
                let source_iter = db.iterator(rocksdb::IteratorMode::From(
                    b"source_ref:",
                    rocksdb::Direction::Forward,
                ));
                let mut keys_to_delete = Vec::new();

                for item in source_iter {
                    let (key, value) = item?;
                    let key_str = String::from_utf8_lossy(&key);
                    if !key_str.starts_with("source_ref:") {
                        break;
                    }

                    if let Ok(reference) = SourceReference::decode(&value[..]) {
                        if let Some(ref scope) = reference.scope {
                            use post_cortex_proto::pb::source_scope::Scope;
                            if let Some(Scope::Function(ref func)) = scope.scope {
                                let ref_key = format!("{}::{}", reference.file_path, func.name);
                                if ref_key == changed_key {
                                    keys_to_delete.push(key.to_vec());
                                    direct_count += 1;
                                } else if dependent_symbols.contains(&ref_key) {
                                    keys_to_delete.push(key.to_vec());
                                    cascade_count += 1;
                                }
                            }
                        }
                    }
                }

                for key in keys_to_delete {
                    db.delete(&key)?;
                }

                Ok(post_cortex_proto::pb::CascadeInvalidateReport {
                    direct_invalidations: direct_count,
                    cascade_invalidations: cascade_count,
                    invalidated_symbols: dependent_symbols,
                })
            },
        )
        .await
        .map_err(|e| anyhow::anyhow!("Task join error: {}", e))?
    }
}
