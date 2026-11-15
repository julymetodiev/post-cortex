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

//! [`FreshnessStore`] trait implementation for [`SurrealDBStorage`].
//!
//! Records are stored in the `source_reference` table with a `status` field
//! (registered 0, stale 1) and the registering `session_id`, following the
//! state machine of [`crate::freshness`]; the stale status is what lets
//! clients detect orphaned symbols after a `register_source_batch`.

use crate::port_error::PortResult;
use async_trait::async_trait;
use serde::Deserialize;
use surrealdb::types::SurrealValue;

use crate::freshness::evaluate;
use post_cortex_core::core::freshness::{
    CascadeInvalidateReport, FreshnessEntry, SourceReference, SourceScope, SymbolId,
};
use post_cortex_ports::storage::{FreshnessStore, InvalidationScope};

use crate::surrealdb_storage::SurrealDBStorage;
use crate::surrealdb_storage::records::{
    SourceReferenceRecord, SymbolDepRecord, source_record_to_reference,
};

#[async_trait]
impl FreshnessStore for SurrealDBStorage {
    async fn register_source(
        &self,
        session_id: uuid::Uuid,
        reference: SourceReference,
    ) -> PortResult<()> {
        self.breaker
            .guard(async {
                let non_empty = |v: &[u8]| (!v.is_empty()).then(|| v.to_vec());
                let (symbol_name, symbol_type, ast_hash, imports) = match reference.function() {
                    Some(func) => (
                        Some(func.name.clone()),
                        (!func.symbol_type.is_empty()).then(|| func.symbol_type.clone()),
                        non_empty(&func.ast_hash),
                        (!func.imports.is_empty()).then(|| func.imports.clone()),
                    ),
                    None => (None, None, None, None),
                };
                let (block_start, block_end, block_hash) = match &reference.scope {
                    SourceScope::Block(block) => (
                        Some(i64::from(block.start_line)),
                        Some(i64::from(block.end_line)),
                        Some(block.content_hash.clone()),
                    ),
                    _ => (None, None, None),
                };

                // Use UPSERT WHERE instead of type::record() to avoid SurrealDB record ID
                // parsing issues with entry_ids containing special chars (/, ::).
                // status = 0 resets any previous stale mark — re-registering a symbol means
                // it is present in the file and its hashes are current.
                let query = "UPSERT source_reference SET \
            entry_id = $entry_id, file_path = $file_path, \
            content_hash = $content_hash, captured_at_unix = $captured_at_unix, \
            symbol_name = $symbol_name, symbol_type = $symbol_type, \
            ast_hash = $ast_hash, imports = $imports, status = 0, \
            block_start = $block_start, block_end = $block_end, block_hash = $block_hash, \
            session_id = $session_id \
            WHERE entry_id = $entry_id;";

                self.db()
                    .query(query)
                    .bind(("entry_id", reference.entry_id))
                    .bind(("file_path", reference.file_path))
                    .bind(("content_hash", reference.content_hash))
                    .bind(("captured_at_unix", reference.captured_at_unix))
                    .bind(("symbol_name", symbol_name))
                    .bind(("symbol_type", symbol_type))
                    .bind(("ast_hash", ast_hash))
                    .bind(("imports", imports))
                    .bind(("block_start", block_start))
                    .bind(("block_end", block_end))
                    .bind(("block_hash", block_hash))
                    .bind(("session_id", session_id.to_string()))
                    .await?
                    .check()?;

                Ok(())
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
                // Mark stale, never delete (see crate::freshness): a stale record
                // reports Stale and is listed as an orphan until registered again.
                // Records without a session predate session tracking and are
                // reached by every scope.
                let mut response = match scope {
                    InvalidationScope::Session(session_id) => self
                        .db()
                        .query(
                            "UPDATE source_reference SET status = 1 WHERE file_path = $file_path \
                         AND (session_id = $session_id OR session_id = NONE);",
                        )
                        .bind(("file_path", file_path.to_string()))
                        .bind(("session_id", session_id.to_string()))
                        .await?
                        .check()?,
                    InvalidationScope::AllSessions => self
                        .db()
                        .query(
                            "UPDATE source_reference SET status = 1 WHERE file_path = $file_path;",
                        )
                        .bind(("file_path", file_path.to_string()))
                        .await?
                        .check()?,
                };
                let updated: Vec<SourceReferenceRecord> = response.take(0)?;
                Ok(updated.len() as u32)
            })
            .await
    }

    async fn get_entries_by_source(&self, file_path: &str) -> PortResult<Vec<SourceReference>> {
        self.breaker
            .guard(async {
                let mut response = self
                    .db()
                    .query("SELECT * FROM source_reference WHERE file_path = $file_path")
                    .bind(("file_path", file_path.to_string()))
                    .await?;

                let records: Vec<SourceReferenceRecord> = response.take(0)?;

                records
                    .into_iter()
                    .map(source_record_to_reference)
                    .collect()
            })
            .await
    }

    async fn get_stale_entries_by_source(
        &self,
        file_path: &str,
    ) -> PortResult<Vec<post_cortex_ports::storage::StaleEntryInfo>> {
        self.breaker
            .guard(async {
                // Only return records that are still explicitly marked stale (status=1).
                // Records re-registered after invalidation will have status=0 and are excluded.
                let mut response = self
                    .db()
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
                    .map(|r| post_cortex_ports::storage::StaleEntryInfo {
                        entry_id: r.entry_id,
                        symbol_name: r.symbol_name,
                        symbol_type: r.symbol_type,
                    })
                    .collect())
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
                // Use query instead of select_one because entry_id contains special chars
                let mut response = self
                    .db()
                    .query("SELECT * FROM source_reference WHERE entry_id = $entry_id LIMIT 1")
                    .bind(("entry_id", entry_id.to_string()))
                    .await?;
                let records: Vec<SourceReferenceRecord> = response.take(0)?;
                let state = records
                    .first()
                    .map(SourceReferenceRecord::state)
                    .transpose()?;
                Ok(evaluate(
                    entry_id.to_string(),
                    state,
                    file_hash.to_vec(),
                    ast_hash,
                ))
            })
            .await
    }

    async fn check_freshness_batch(
        &self,
        entries: Vec<(String, Vec<u8>, Option<Vec<u8>>, Option<String>)>,
    ) -> PortResult<Vec<FreshnessEntry>> {
        self.breaker
            .guard(async {
                if entries.is_empty() {
                    return Ok(Vec::new());
                }

                // Collect the entry_ids for the single batch query.
                let ids: Vec<String> = entries.iter().map(|(id, _, _, _)| id.clone()).collect();

                // ONE round-trip to SurrealDB: WHERE entry_id IN $ids.
                // We use .query()/.bind() to avoid the record-ID parsing issue with
                // entry_ids that contain '/' or '::'.
                let mut response = self
                    .db()
                    .query("SELECT * FROM source_reference WHERE entry_id IN $ids")
                    .bind(("ids", ids.clone()))
                    .await?;
                let records: Vec<SourceReferenceRecord> = response.take(0)?;

                // Build a lookup map: entry_id -> record
                let record_map: std::collections::HashMap<String, SourceReferenceRecord> = records
                    .into_iter()
                    .map(|r| (r.entry_id.clone(), r))
                    .collect();

                entries
                    .into_iter()
                    .map(|(entry_id, file_hash, ast_hash, _symbol_name)| {
                        let state = record_map
                            .get(&entry_id)
                            .map(SourceReferenceRecord::state)
                            .transpose()?;
                        Ok(evaluate(entry_id, state, file_hash, ast_hash.as_deref()))
                    })
                    .collect()
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
                    self.db()
                        .query(query)
                        .bind(("from_key", from_key))
                        .bind(("from_file", from.file_path.clone()))
                        .bind(("from_symbol", from.symbol_name.clone()))
                        .bind(("to_file", to.file_path.clone()))
                        .bind(("to_symbol", to.symbol_name.clone()))
                        .bind(("to_type", to.symbol_type.clone()))
                        .await?
                        .check()?;
                    count += 1;
                }
                Ok(count)
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
                        .db()
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
                    .db()
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
                            .db()
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
            })
            .await
    }
}
