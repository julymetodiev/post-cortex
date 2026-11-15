// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! Source-freshness gRPC methods (Phase 9).
//!
//! Implemented as inherent `*_impl` methods on `PcxGrpcService`; the trait
//! impl in `mod.rs` thinly delegates here so the freshness surface can grow
//! without bloating the main service file.

use crate::grpc::status;
use tonic::{Request, Response, Status};
use tracing::{debug, error};

use crate::memory::helpers::{get_session_id_from_metadata, parse_uuid};
use post_cortex_memory::Memory;
use post_cortex_proto::pb::{
    CascadeInvalidateReport, CascadeInvalidateRequest, FreshnessReport, FreshnessRequest,
    GetStaleEntriesBySourceRequest, GetStaleEntriesBySourceResponse, InvalidateAck,
    InvalidateRequest, RegisterSourceAck, RegisterSourceBatchAck, RegisterSourceBatchRequest,
    RegisterSourceRequest, RegisterSymbolDependencyAck, RegisterSymbolDependencyRequest,
    StaleEntryInfo,
};

mod wire;

pub use wire::source_reference;

/// The session of a source registration: the request's `session_id`, else
/// (older clients) the `x-session-id` metadata; neither is an invalid
/// argument.
fn session_of<T>(request: &Request<T>, field: &str) -> Result<uuid::Uuid, Status> {
    match field.trim() {
        "" => parse_uuid(&get_session_id_from_metadata(request)?),
        id => parse_uuid(id),
    }
}

pub async fn register_source_impl(
    memory: &Memory,
    request: Request<RegisterSourceRequest>,
) -> Result<Response<RegisterSourceAck>, Status> {
    let session_id = session_of(&request, &request.get_ref().session_id)?;
    let req = request.into_inner();

    // Ensure source_ref is present
    let source_ref = req
        .source_ref
        .ok_or_else(|| Status::invalid_argument("Missing source_ref in request"))?;

    debug!("gRPC RegisterSource: entry_id={}", source_ref.entry_id);

    match memory
        .freshness
        .register(session_id, wire::reference(source_ref))
        .await
    {
        Ok(()) => Ok(Response::new(RegisterSourceAck {})),
        Err(e) => {
            error!("gRPC RegisterSource failed: {}", e);
            Err(status(e))
        }
    }
}

pub async fn register_source_batch_impl(
    memory: &Memory,
    request: Request<RegisterSourceBatchRequest>,
) -> Result<Response<RegisterSourceBatchAck>, Status> {
    let session_id = session_of(&request, &request.get_ref().session_id)?;
    let req = request.into_inner();
    let total = req.sources.len();
    debug!(
        "gRPC RegisterSourceBatch: session={} sources={}",
        session_id, total
    );

    // Bounded inside the service: not one storage request per source at
    // once.
    let registered = memory
        .freshness
        .register_batch(
            session_id,
            req.sources.into_iter().map(wire::reference).collect(),
        )
        .await;

    debug!(
        "gRPC RegisterSourceBatch: registered {}/{} sources",
        registered, total
    );
    Ok(Response::new(RegisterSourceBatchAck { registered }))
}

pub async fn check_freshness_impl(
    memory: &Memory,
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

    if !req.checks.is_empty() && req.checks.len() != req.entry_ids.len() {
        return Err(Status::invalid_argument(
            "checks must be empty or as long as entry_ids",
        ));
    }
    let has_checks = !req.checks.is_empty();

    // Build the batch input: one tuple per entry.
    let batch: Vec<post_cortex_memory::capabilities::FreshnessCheck> = req
        .entry_ids
        .iter()
        .enumerate()
        .map(|(i, entry_id)| {
            let current_hash = req.current_hashes[i].hash.clone();

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

    // One port call: one SurrealDB round trip for the whole batch.
    match memory.freshness.check_batch(batch).await {
        Ok(entries) => Ok(Response::new(FreshnessReport {
            entries: entries.into_iter().map(wire::entry).collect(),
        })),
        Err(e) => {
            error!("gRPC CheckFreshness batch failed: {}", e);
            Err(status(e))
        }
    }
}

pub async fn invalidate_impl(
    memory: &Memory,
    request: Request<InvalidateRequest>,
) -> Result<Response<InvalidateAck>, Status> {
    let req = request.into_inner();
    debug!("gRPC Invalidate: checking source path {}", req.file_path);

    // If session_id is provided, also rebuild entity graph
    if !req.session_id.is_empty() {
        let session_id = uuid::Uuid::parse_str(&req.session_id)
            .map_err(|e| Status::invalid_argument(format!("Invalid session_id: {}", e)))?;

        match memory
            .freshness
            .invalidate(session_id, &req.file_path)
            .await
        {
            Ok((entries_invalidated, entities_after)) => Ok(Response::new(InvalidateAck {
                entries_invalidated,
                entities_rebuilt: entities_after as u32,
            })),
            Err(e) => {
                error!(
                    "gRPC Invalidate+rebuild failed for file {}: {}",
                    req.file_path, e
                );
                Err(status(e))
            }
        }
    } else {
        // No session: the explicit administrative invalidation of every
        // session's references to the file (no records are removed).
        match memory
            .freshness
            .invalidate_all_sessions(&req.file_path)
            .await
        {
            Ok(count) => Ok(Response::new(InvalidateAck {
                entries_invalidated: count,
                entities_rebuilt: 0,
            })),
            Err(e) => {
                error!("gRPC Invalidate failed for file {}: {}", req.file_path, e);
                Err(status(e))
            }
        }
    }
}

pub async fn register_symbol_dependency_impl(
    memory: &Memory,
    request: Request<RegisterSymbolDependencyRequest>,
) -> Result<Response<RegisterSymbolDependencyAck>, Status> {
    let req = request.into_inner();
    let from = wire::symbol(
        req.from_symbol
            .ok_or_else(|| Status::invalid_argument("Missing from_symbol"))?,
    );
    let to_symbols: Vec<_> = req.to_symbols.into_iter().map(wire::symbol).collect();

    debug!(
        "gRPC RegisterSymbolDependency: {}::{} -> {} deps",
        from.file_path,
        from.symbol_name,
        to_symbols.len()
    );

    match memory
        .freshness
        .register_symbol_dependencies(from, to_symbols)
        .await
    {
        Ok(count) => Ok(Response::new(RegisterSymbolDependencyAck {
            edges_created: count,
        })),
        Err(e) => {
            error!("gRPC RegisterSymbolDependency failed: {}", e);
            Err(status(e))
        }
    }
}

pub async fn cascade_invalidate_impl(
    memory: &Memory,
    request: Request<CascadeInvalidateRequest>,
) -> Result<Response<CascadeInvalidateReport>, Status> {
    let req = request.into_inner();
    let changed = wire::symbol(
        req.changed_symbol
            .ok_or_else(|| Status::invalid_argument("Missing changed_symbol"))?,
    );
    let max_depth = if req.max_depth == 0 {
        10
    } else {
        req.max_depth
    };

    debug!(
        "gRPC CascadeInvalidate: {}::{} depth={}",
        changed.file_path, changed.symbol_name, max_depth
    );

    match memory
        .freshness
        .cascade_invalidate(changed, req.new_ast_hash, max_depth)
        .await
    {
        Ok(report) => Ok(Response::new(wire::cascade_report(report))),
        Err(e) => {
            error!("gRPC CascadeInvalidate failed: {}", e);
            Err(status(e))
        }
    }
}

pub async fn get_stale_entries_by_source_impl(
    memory: &Memory,
    request: Request<GetStaleEntriesBySourceRequest>,
) -> Result<Response<GetStaleEntriesBySourceResponse>, Status> {
    let req = request.into_inner();
    debug!("gRPC GetStaleEntriesBySource: file_path={}", req.file_path);

    match memory.freshness.stale_entries(&req.file_path).await {
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
            Err(status(e))
        }
    }
}
