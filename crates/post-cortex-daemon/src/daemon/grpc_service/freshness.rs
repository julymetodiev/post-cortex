// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! Source-freshness gRPC methods (Phase 9).
//!
//! Implemented as inherent `*_impl` methods on `PcxGrpcService`; the trait
//! impl in `mod.rs` thinly delegates here so the freshness surface can grow
//! without bloating the main service file.

use futures::future::join_all;
use tonic::{Request, Response, Status};
use tracing::{debug, error};

use super::PcxGrpcService;
use super::helpers::{get_session_id_from_metadata, parse_uuid};
use super::pb::{
    CascadeInvalidateReport, CascadeInvalidateRequest, FreshnessEntry, FreshnessReport,
    FreshnessRequest, FreshnessStatus, GetStaleEntriesBySourceRequest,
    GetStaleEntriesBySourceResponse, InvalidateAck, InvalidateRequest, RegisterSourceAck,
    RegisterSourceBatchAck, RegisterSourceBatchRequest, RegisterSourceRequest,
    RegisterSymbolDependencyAck, RegisterSymbolDependencyRequest, StaleEntryInfo,
};

impl PcxGrpcService {
    pub(super) async fn register_source_impl(
        &self,
        request: Request<RegisterSourceRequest>,
    ) -> Result<Response<RegisterSourceAck>, Status> {
        let session_id_str = get_session_id_from_metadata(&request)?;
        let session_id = parse_uuid(&session_id_str)?;

        let req = request.into_inner();

        // Ensure source_ref is present
        let source_ref = req
            .source_ref
            .ok_or_else(|| Status::invalid_argument("Missing source_ref in request"))?;

        debug!("gRPC RegisterSource: entry_id={}", source_ref.entry_id);

        match self
            .memory
            .storage_actor
            .register_source(session_id, source_ref)
            .await
        {
            Ok(()) => Ok(Response::new(RegisterSourceAck {})),
            Err(e) => {
                error!("gRPC RegisterSource failed: {}", e);
                let e_msg: String = e.to_string();
                Err(Status::internal(e_msg))
            }
        }
    }

    pub(super) async fn register_source_batch_impl(
        &self,
        request: Request<RegisterSourceBatchRequest>,
    ) -> Result<Response<RegisterSourceBatchAck>, Status> {
        let session_id_str = get_session_id_from_metadata(&request)?;
        let session_id = parse_uuid(&session_id_str)?;

        let req = request.into_inner();
        let total = req.sources.len();
        debug!(
            "gRPC RegisterSourceBatch: session={} sources={}",
            session_id, total
        );

        let futures = req.sources.into_iter().map(|source_ref| {
            let entry_id = source_ref.entry_id.clone();
            let actor = self.memory.storage_actor.clone();
            async move { (entry_id, actor.register_source(session_id, source_ref).await) }
        });
        let results = join_all(futures).await;

        let mut registered: u32 = 0;
        for (entry_id, result) in results {
            match result {
                Ok(()) => registered += 1,
                Err(e) => {
                    error!(
                        "gRPC RegisterSourceBatch: failed for entry_id={}: {}",
                        entry_id, e
                    );
                }
            }
        }

        debug!(
            "gRPC RegisterSourceBatch: registered {}/{} sources",
            registered, total
        );
        Ok(Response::new(RegisterSourceBatchAck { registered }))
    }

    pub(super) async fn check_freshness_impl(
        &self,
        request: Request<FreshnessRequest>,
    ) -> Result<Response<FreshnessReport>, Status> {
        let req = request.into_inner();
        debug!(
            "gRPC CheckFreshness: checking {} entries",
            req.entry_ids.len()
        );

        if req.entry_ids.len() != req.current_hashes.len() {
            return Err(Status::invalid_argument(
                "entry_ids and current_hashes must have the same length",
            ));
        }

        let has_checks = req.checks.len() == req.entry_ids.len();

        // Collect per-entry metadata needed for fallback Unknown responses.
        // (file_path is not stored in storage; it comes from the request.)
        let mut file_paths: Vec<String> = Vec::with_capacity(req.entry_ids.len());
        let mut fallback_hashes: Vec<Vec<u8>> = Vec::with_capacity(req.entry_ids.len());

        // Build the batch input: one tuple per entry.
        let batch: Vec<(String, Vec<u8>, Option<Vec<u8>>, Option<String>)> = req
            .entry_ids
            .iter()
            .enumerate()
            .map(|(i, entry_id)| {
                let current_hash = req.current_hashes[i].hash.clone();
                file_paths.push(req.current_hashes[i].file_path.clone());
                fallback_hashes.push(current_hash.clone());

                let (ast_hash, symbol_name) = if has_checks {
                    let check = &req.checks[i];
                    (
                        if check.ast_hash.is_empty() {
                            None
                        } else {
                            Some(check.ast_hash.clone())
                        },
                        if check.symbol_name.is_empty() {
                            None
                        } else {
                            Some(check.symbol_name.clone())
                        },
                    )
                } else {
                    (None, None)
                };

                (entry_id.clone(), current_hash, ast_hash, symbol_name)
            })
            .collect();

        // Single actor message — one SurrealDB round-trip for the whole batch.
        match self.memory.storage_actor.check_freshness_batch(batch).await {
            Ok(entries) => Ok(Response::new(FreshnessReport { entries })),
            Err(e) => {
                error!("gRPC CheckFreshness batch failed: {}", e);
                // Fall back to Unknown for every entry so callers are not blocked.
                let reports = req
                    .entry_ids
                    .into_iter()
                    .enumerate()
                    .map(|(i, entry_id)| FreshnessEntry {
                        entry_id,
                        file_path: file_paths.get(i).cloned().unwrap_or_default(),
                        status: FreshnessStatus::Unknown as i32,
                        stored_hash: Vec::new(),
                        current_hash: fallback_hashes.get(i).cloned().unwrap_or_default(),
                    })
                    .collect();
                Ok(Response::new(FreshnessReport { entries: reports }))
            }
        }
    }

    pub(super) async fn invalidate_impl(
        &self,
        request: Request<InvalidateRequest>,
    ) -> Result<Response<InvalidateAck>, Status> {
        let req = request.into_inner();
        debug!("gRPC Invalidate: checking source path {}", req.file_path);

        // If session_id is provided, also rebuild entity graph
        if !req.session_id.is_empty() {
            let session_id = uuid::Uuid::parse_str(&req.session_id)
                .map_err(|e| Status::invalid_argument(format!("Invalid session_id: {}", e)))?;

            match self
                .memory
                .invalidate_and_rebuild_entity_graph(session_id, &req.file_path)
                .await
            {
                Ok((entries_invalidated, entities_after)) => {
                    Ok(Response::new(InvalidateAck {
                        entries_invalidated,
                        entities_rebuilt: entities_after as u32,
                    }))
                }
                Err(e) => {
                    error!(
                        "gRPC Invalidate+rebuild failed for file {}: {}",
                        req.file_path, e
                    );
                    Err(Status::internal(e.to_string()))
                }
            }
        } else {
            // Legacy path: only invalidate source references
            match self
                .memory
                .storage_actor
                .invalidate_source(&req.file_path)
                .await
            {
                Ok(count) => Ok(Response::new(InvalidateAck {
                    entries_invalidated: count,
                    entities_rebuilt: 0,
                })),
                Err(e) => {
                    error!("gRPC Invalidate failed for file {}: {}", req.file_path, e);
                    Err(Status::internal(e.to_string()))
                }
            }
        }
    }

    pub(super) async fn register_symbol_dependency_impl(
        &self,
        request: Request<RegisterSymbolDependencyRequest>,
    ) -> Result<Response<RegisterSymbolDependencyAck>, Status> {
        let req = request.into_inner();
        let from = req
            .from_symbol
            .ok_or_else(|| Status::invalid_argument("Missing from_symbol"))?;
        let to_symbols = req.to_symbols;

        debug!(
            "gRPC RegisterSymbolDependency: {}::{} -> {} deps",
            from.file_path,
            from.symbol_name,
            to_symbols.len()
        );

        match self
            .memory
            .storage_actor
            .register_symbol_dependencies(from, to_symbols)
            .await
        {
            Ok(count) => Ok(Response::new(RegisterSymbolDependencyAck {
                edges_created: count,
            })),
            Err(e) => {
                error!("gRPC RegisterSymbolDependency failed: {}", e);
                Err(Status::internal(e.to_string()))
            }
        }
    }

    pub(super) async fn cascade_invalidate_impl(
        &self,
        request: Request<CascadeInvalidateRequest>,
    ) -> Result<Response<CascadeInvalidateReport>, Status> {
        let req = request.into_inner();
        let changed = req
            .changed_symbol
            .ok_or_else(|| Status::invalid_argument("Missing changed_symbol"))?;
        let max_depth = if req.max_depth == 0 { 10 } else { req.max_depth };

        debug!(
            "gRPC CascadeInvalidate: {}::{} depth={}",
            changed.file_path, changed.symbol_name, max_depth
        );

        match self
            .memory
            .storage_actor
            .cascade_invalidate(changed, req.new_ast_hash, max_depth)
            .await
        {
            Ok(report) => Ok(Response::new(report)),
            Err(e) => {
                error!("gRPC CascadeInvalidate failed: {}", e);
                Err(Status::internal(e.to_string()))
            }
        }
    }

    pub(super) async fn get_stale_entries_by_source_impl(
        &self,
        request: Request<GetStaleEntriesBySourceRequest>,
    ) -> Result<Response<GetStaleEntriesBySourceResponse>, Status> {
        let req = request.into_inner();
        debug!(
            "gRPC GetStaleEntriesBySource: file_path={}",
            req.file_path
        );

        match self
            .memory
            .storage_actor
            .get_stale_entries_by_source(&req.file_path)
            .await
        {
            Ok(stale) => {
                let entries = stale
                    .into_iter()
                    .map(|s| StaleEntryInfo {
                        entry_id: s.entry_id,
                        symbol_name: s.symbol_name.unwrap_or_default(),
                        symbol_type: s.symbol_type.unwrap_or_default(),
                    })
                    .collect();
                Ok(Response::new(GetStaleEntriesBySourceResponse { entries }))
            }
            Err(e) => {
                error!("gRPC GetStaleEntriesBySource failed: {}", e);
                Err(Status::internal(e.to_string()))
            }
        }
    }
}
