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

//! [`FreshnessStorage`] trait implementation for [`SurrealDBStorage`].
//!
//! Records are stored in the `source_reference` table with a `status` field
//! that distinguishes Fresh (0) from explicitly invalidated (1) — that's what
//! lets us detect orphaned symbols after a `register_source_batch`.

use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use surrealdb::types::SurrealValue;

use crate::traits::FreshnessStorage;
use post_cortex_proto::pb::{
    CascadeInvalidateReport, FreshnessEntry, FreshnessStatus, SourceReference, SymbolId,
};

use super::SurrealDBStorage;
use super::records::{SourceReferenceRecord, SymbolDepRecord, source_record_to_reference};

#[async_trait]
impl FreshnessStorage for SurrealDBStorage {
    async fn register_source(
        &self,
        _session_id: uuid::Uuid,
        reference: SourceReference,
    ) -> Result<()> {
        // Extract scope info if present
        let (symbol_name, symbol_type, ast_hash, imports) = if let Some(ref scope) = reference.scope
        {
            use post_cortex_proto::pb::source_scope::Scope;
            match &scope.scope {
                Some(Scope::Function(func)) => (
                    Some(func.name.clone()),
                    if func.symbol_type.is_empty() {
                        None
                    } else {
                        Some(func.symbol_type.clone())
                    },
                    if func.ast_hash.is_empty() {
                        None
                    } else {
                        Some(func.ast_hash.clone())
                    },
                    if func.imports.is_empty() {
                        None
                    } else {
                        Some(func.imports.clone())
                    },
                ),
                _ => (None, None, None, None),
            }
        } else {
            (None, None, None, None)
        };

        // Use UPSERT WHERE instead of type::record() to avoid SurrealDB record ID
        // parsing issues with entry_ids containing special chars (/, ::).
        // status = 0 resets any previous stale mark — re-registering a symbol means
        // it is present in the file and its hashes are current.
        let query = "UPSERT source_reference SET \
            entry_id = $entry_id, file_path = $file_path, \
            content_hash = $content_hash, captured_at_unix = $captured_at_unix, \
            symbol_name = $symbol_name, symbol_type = $symbol_type, \
            ast_hash = $ast_hash, imports = $imports, status = 0 \
            WHERE entry_id = $entry_id;";

        self.db
            .query(query)
            .bind(("entry_id", reference.entry_id))
            .bind(("file_path", reference.file_path))
            .bind(("content_hash", reference.content_hash))
            .bind(("captured_at_unix", reference.captured_at_unix))
            .bind(("symbol_name", symbol_name))
            .bind(("symbol_type", symbol_type))
            .bind(("ast_hash", ast_hash))
            .bind(("imports", imports))
            .await?;

        Ok(())
    }

    async fn check_freshness(&self, entry_id: &str, file_hash: &[u8]) -> Result<FreshnessEntry> {
        // Use query instead of select_one because entry_id contains special chars
        // (slashes, colons) that SurrealDB record IDs handle differently than UPSERT.
        let mut response = self
            .db
            .query("SELECT * FROM source_reference WHERE entry_id = $entry_id LIMIT 1")
            .bind(("entry_id", entry_id.to_string()))
            .await?;
        let records: Vec<SourceReferenceRecord> = response.take(0)?;
        let record = records.into_iter().next();
        let current_hash = file_hash.to_vec();

        match record {
            Some(r) => {
                // Explicitly invalidated records short-circuit to Stale immediately.
                if r.status == 1 {
                    return Ok(FreshnessEntry {
                        entry_id: r.entry_id,
                        file_path: r.file_path,
                        status: FreshnessStatus::Stale as i32,
                        stored_hash: r.content_hash,
                        current_hash,
                    });
                }

                let is_fresh = r.content_hash == current_hash;

                let status = if is_fresh {
                    FreshnessStatus::Fresh as i32
                } else {
                    FreshnessStatus::Stale as i32
                };

                Ok(FreshnessEntry {
                    entry_id: r.entry_id,
                    file_path: r.file_path,
                    status,
                    stored_hash: r.content_hash,
                    current_hash,
                })
            }
            None => Ok(FreshnessEntry {
                entry_id: entry_id.to_string(),
                file_path: String::new(),
                status: FreshnessStatus::Unknown as i32,
                stored_hash: Vec::new(),
                current_hash,
            }),
        }
    }

    async fn invalidate_source(&self, file_path: &str) -> Result<u32> {
        // Mark stale rather than delete so that:
        // 1. check_freshness_* can return Stale (status=1) instead of Unknown.
        // 2. Cascade invalidation still triggers for changed symbols.
        // 3. Symbols that are deleted from the file stay as status=1 (not re-registered
        //    by register_source_batch) and can be detected as orphans.
        let query = "UPDATE source_reference SET status = 1 WHERE file_path = $file_path;";
        let mut response = self
            .db
            .query(query)
            .bind(("file_path", file_path.to_string()))
            .await?;
        let updated: Vec<SourceReferenceRecord> = response.take(0)?;
        Ok(updated.len() as u32)
    }

    async fn get_entries_by_source(&self, file_path: &str) -> Result<Vec<SourceReference>> {
        let mut response = self
            .db
            .query("SELECT * FROM source_reference WHERE file_path = $file_path")
            .bind(("file_path", file_path.to_string()))
            .await?;

        let records: Vec<SourceReferenceRecord> = response.take(0)?;

        let references = records
            .into_iter()
            .map(source_record_to_reference)
            .collect();

        Ok(references)
    }

    async fn get_stale_entries_by_source(
        &self,
        file_path: &str,
    ) -> Result<Vec<crate::traits::StaleEntryInfo>> {
        // Only return records that are still explicitly marked stale (status=1).
        // Records re-registered after invalidation will have status=0 and are excluded.
        let mut response = self
            .db
            .query(
                "SELECT entry_id, symbol_name, symbol_type FROM source_reference WHERE file_path = $file_path AND status = 1",
            )
            .bind(("file_path", file_path.to_string()))
            .await?;

        #[derive(Debug, Clone, serde::Serialize, Deserialize, SurrealValue)]
        struct StaleRow {
            entry_id: String,
            #[serde(default)]
            symbol_name: Option<String>,
            #[serde(default)]
            symbol_type: Option<String>,
        }

        let rows: Vec<StaleRow> = response.take(0)?;
        Ok(rows
            .into_iter()
            .map(|r| crate::traits::StaleEntryInfo {
                entry_id: r.entry_id,
                symbol_name: r.symbol_name,
                symbol_type: r.symbol_type,
            })
            .collect())
    }

    async fn check_freshness_semantic(
        &self,
        entry_id: &str,
        file_hash: &[u8],
        ast_hash: Option<&[u8]>,
        _symbol_name: Option<&str>,
    ) -> Result<FreshnessEntry> {
        // Use query instead of select_one because entry_id contains special chars
        let mut response = self
            .db
            .query("SELECT * FROM source_reference WHERE entry_id = $entry_id LIMIT 1")
            .bind(("entry_id", entry_id.to_string()))
            .await?;
        let records: Vec<SourceReferenceRecord> = response.take(0)?;
        let record = records.into_iter().next();
        let current_hash = file_hash.to_vec();

        match record {
            Some(r) => {
                // If the record was explicitly marked stale by invalidate_source or
                // cascade_invalidate (status=1), short-circuit immediately — no need
                // to compare hashes.  The re-registration path resets status to 0.
                if r.status == 1 {
                    return Ok(FreshnessEntry {
                        entry_id: r.entry_id,
                        file_path: r.file_path,
                        status: FreshnessStatus::Stale as i32,
                        stored_hash: r.content_hash,
                        current_hash,
                    });
                }

                // Semantic: if both sides have ast_hash, compare that
                let is_fresh = if let (Some(client_ast), Some(stored_ast)) = (ast_hash, &r.ast_hash)
                {
                    client_ast == stored_ast.as_slice()
                } else {
                    r.content_hash == current_hash
                };

                let status = if is_fresh {
                    FreshnessStatus::Fresh as i32
                } else {
                    FreshnessStatus::Stale as i32
                };

                Ok(FreshnessEntry {
                    entry_id: r.entry_id,
                    file_path: r.file_path,
                    status,
                    stored_hash: r.content_hash,
                    current_hash,
                })
            }
            None => Ok(FreshnessEntry {
                entry_id: entry_id.to_string(),
                file_path: String::new(),
                status: FreshnessStatus::Unknown as i32,
                stored_hash: Vec::new(),
                current_hash,
            }),
        }
    }

    async fn check_freshness_batch(
        &self,
        entries: Vec<(String, Vec<u8>, Option<Vec<u8>>, Option<String>)>,
    ) -> Result<Vec<FreshnessEntry>> {
        if entries.is_empty() {
            return Ok(Vec::new());
        }

        // Collect the entry_ids for the single batch query.
        let ids: Vec<String> = entries.iter().map(|(id, _, _, _)| id.clone()).collect();

        // ONE round-trip to SurrealDB: WHERE entry_id IN $ids.
        // We use .query()/.bind() to avoid the record-ID parsing issue with
        // entry_ids that contain '/' or '::'.
        let mut response = self
            .db
            .query("SELECT * FROM source_reference WHERE entry_id IN $ids")
            .bind(("ids", ids.clone()))
            .await?;
        let records: Vec<SourceReferenceRecord> = response.take(0)?;

        // Build a lookup map: entry_id -> record
        let record_map: std::collections::HashMap<String, SourceReferenceRecord> = records
            .into_iter()
            .map(|r| (r.entry_id.clone(), r))
            .collect();

        // For each input entry, match against the fetched records and compare hashes.
        let mut results = Vec::with_capacity(entries.len());
        for (entry_id, file_hash, ast_hash, _symbol_name) in entries {
            let current_hash = file_hash.clone();
            match record_map.get(&entry_id) {
                Some(r) => {
                    // If explicitly marked stale by invalidate_source / cascade_invalidate,
                    // short-circuit — no hash comparison needed.
                    if r.status == 1 {
                        results.push(FreshnessEntry {
                            entry_id: r.entry_id.clone(),
                            file_path: r.file_path.clone(),
                            status: FreshnessStatus::Stale as i32,
                            stored_hash: r.content_hash.clone(),
                            current_hash,
                        });
                        continue;
                    }

                    // Semantic: prefer ast_hash comparison when both sides have it
                    let is_fresh = if let (Some(client_ast), Some(stored_ast)) =
                        (ast_hash.as_deref(), r.ast_hash.as_deref())
                    {
                        client_ast == stored_ast
                    } else {
                        r.content_hash == current_hash
                    };

                    let status = if is_fresh {
                        FreshnessStatus::Fresh as i32
                    } else {
                        FreshnessStatus::Stale as i32
                    };

                    results.push(FreshnessEntry {
                        entry_id: r.entry_id.clone(),
                        file_path: r.file_path.clone(),
                        status,
                        stored_hash: r.content_hash.clone(),
                        current_hash,
                    });
                }
                None => {
                    results.push(FreshnessEntry {
                        entry_id,
                        file_path: String::new(),
                        status: FreshnessStatus::Unknown as i32,
                        stored_hash: Vec::new(),
                        current_hash,
                    });
                }
            }
        }

        Ok(results)
    }

    async fn register_symbol_dependencies(
        &self,
        from: SymbolId,
        to_symbols: Vec<SymbolId>,
    ) -> Result<u32> {
        let mut count = 0u32;
        for to in &to_symbols {
            // Use SurrealDB native graph relations
            let query = "UPSERT type::record('symbol_dep', $from_key) SET \
                from_file = $from_file, from_symbol = $from_symbol, \
                to_file = $to_file, to_symbol = $to_symbol, \
                to_symbol_type = $to_type;";
            let from_key = format!(
                "{}::{}->{}::{}",
                from.file_path, from.symbol_name, to.file_path, to.symbol_name
            );
            self.db
                .query(query)
                .bind(("from_key", from_key))
                .bind(("from_file", from.file_path.clone()))
                .bind(("from_symbol", from.symbol_name.clone()))
                .bind(("to_file", to.file_path.clone()))
                .bind(("to_symbol", to.symbol_name.clone()))
                .bind(("to_type", to.symbol_type.clone()))
                .await?;
            count += 1;
        }
        Ok(count)
    }

    async fn cascade_invalidate(
        &self,
        changed: SymbolId,
        _new_ast_hash: Vec<u8>,
        max_depth: u32,
    ) -> Result<CascadeInvalidateReport> {
        use std::collections::{HashSet, VecDeque};

        // BFS through symbol_dep to find all dependents
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();
        let changed_key = format!("{}::{}", changed.file_path, changed.symbol_name);
        queue.push_back((changed.file_path.clone(), changed.symbol_name.clone(), 0u32));
        visited.insert(changed_key.clone());

        let mut dependent_symbols = Vec::new();

        while let Some((file, sym, depth)) = queue.pop_front() {
            if depth > 0 {
                dependent_symbols.push(format!("{}::{}", file, sym));
            }
            if depth >= max_depth {
                continue;
            }

            // Find who depends on this symbol (reverse lookup)
            // Match by exact file+symbol, OR by symbol name only (when dep was registered without file)
            let query = "SELECT * FROM symbol_dep WHERE \
                (to_file = $to_file AND to_symbol = $to_symbol) OR \
                (to_file = '' AND to_symbol = $to_symbol);";
            let mut response = self
                .db
                .query(query)
                .bind(("to_file", file.clone()))
                .bind(("to_symbol", sym.clone()))
                .await?;

            let deps: Vec<SymbolDepRecord> = response.take(0)?;
            for dep in deps {
                let dep_key = format!("{}::{}", dep.from_file, dep.from_symbol);
                if visited.insert(dep_key) {
                    queue.push_back((dep.from_file, dep.from_symbol, depth + 1));
                }
            }
        }

        // Mark source_reference entries stale rather than deleting them.
        // This preserves baseline records so freshness checks return Stale (status=1)
        // instead of Unknown (no record), which is what triggers cascade invalidation.

        let mut cascade_count = 0u32;

        // Direct: mark entries for the changed symbol as stale
        let query = "UPDATE source_reference SET status = 1 WHERE file_path = $file AND symbol_name = $symbol;";
        let mut response = self
            .db
            .query(query)
            .bind(("file", changed.file_path.clone()))
            .bind(("symbol", changed.symbol_name.clone()))
            .await?;
        let updated: Vec<SourceReferenceRecord> = response.take(0)?;
        let direct_count: u32 = updated.len() as u32;

        // Cascade: mark entries for dependent symbols as stale
        for dep_key in &dependent_symbols {
            if let Some((file, sym)) = dep_key.split_once("::") {
                let query = "UPDATE source_reference SET status = 1 WHERE file_path = $file AND symbol_name = $symbol;";
                let mut response = self
                    .db
                    .query(query)
                    .bind(("file", file.to_string()))
                    .bind(("symbol", sym.to_string()))
                    .await?;
                let updated: Vec<SourceReferenceRecord> = response.take(0)?;
                cascade_count += updated.len() as u32;
            }
        }

        Ok(CascadeInvalidateReport {
            direct_invalidations: direct_count,
            cascade_invalidations: cascade_count,
            invalidated_symbols: dependent_symbols,
        })
    }
}
