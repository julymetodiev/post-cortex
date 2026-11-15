// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! gRPC record methods: writes, bulk writes, entity deletes and merges,
//! forget and purge, checkpoints and structured queries.

use crate::grpc::status;
use crate::memory::helpers::{parse_uuid, proto_bulk_item_to_request, proto_update_to_request};
use tonic::{Request, Response, Status};
use tracing::{debug, error, info};

use post_cortex_core::core::context_update::UpdateType;
use post_cortex_memory::Memory;
use post_cortex_memory::capabilities::ListRecords;
use post_cortex_proto::pb::*;

/// `failed` from `what`, logged by whose fault it is: a request refused as it
/// is, as a foreign `supersedes`, is the caller's and no error of the server.
fn logged(what: &str, failed: Status) -> Status {
    if failed.code() == tonic::Code::InvalidArgument {
        info!("gRPC {what} refused: {}", failed.message());
    } else {
        error!("gRPC {what} failed: {}", failed.message());
    }
    failed
}

pub async fn update_context(
    memory: &Memory,
    request: Request<UpdateContextRequest>,
) -> Result<Response<UpdateContextResponse>, Status> {
    let req = request.into_inner();
    debug!(
        "gRPC UpdateContext: session={}, type={}",
        req.session_id, req.interaction_type
    );

    let service_req = proto_update_to_request(req)?;
    let resp = memory
        .records
        .write(service_req)
        .await
        .map_err(|e| logged("UpdateContext", status(e)))?;

    Ok(Response::new(UpdateContextResponse {
        update_id: resp.entry_id.to_string(),
        success: resp.durable,
    }))
}

pub async fn bulk_update_context(
    memory: &Memory,
    request: Request<BulkUpdateContextRequest>,
) -> Result<Response<BulkUpdateContextResponse>, Status> {
    let req = request.into_inner();
    debug!(
        "gRPC BulkUpdateContext: session={}, count={}",
        req.session_id,
        req.updates.len()
    );

    // One bulk write, as MCP: every item is translated and validated
    // before anything is stored, so a bad item writes nothing, and the
    // ids come back in input order (`update_ids[i]` is `updates[i]`).
    let session_id = parse_uuid(&req.session_id)?;
    let updates = req
        .updates
        .into_iter()
        .enumerate()
        .map(|(i, item)| {
            proto_bulk_item_to_request(session_id, item, req.author.clone())
                .map_err(|s| Status::new(s.code(), format!("updates[{i}]: {}", s.message())))
        })
        .collect::<Result<Vec<_>, Status>>()?;
    let resp = memory
        .records
        .bulk_write(post_cortex_core::services::BulkUpdateContextRequest {
            session_id,
            updates,
        })
        .await
        .map_err(|e| logged("BulkUpdateContext", status(e)))?;
    let success_count = resp.entry_ids.len() as u32;
    Ok(Response::new(BulkUpdateContextResponse {
        update_ids: resp.entry_ids.iter().map(ToString::to_string).collect(),
        success_count,
        failure_count: 0,
    }))
}

pub async fn delete_entity(
    memory: &Memory,
    request: Request<DeleteEntityRequest>,
) -> Result<Response<DeleteEntityResponse>, Status> {
    let req = request.into_inner();
    debug!(
        "gRPC DeleteEntity: session_id={} entity={}",
        req.session_id, req.entity_name
    );

    let session_id = parse_uuid(&req.session_id)?;

    match memory.entities.delete(session_id, &req.entity_name).await {
        Ok(existed) => Ok(Response::new(DeleteEntityResponse { existed })),
        Err(e) => {
            error!("gRPC DeleteEntity failed: {}", e);
            Err(status(e))
        }
    }
}

pub async fn merge_entities(
    memory: &Memory,
    request: Request<MergeEntitiesRequest>,
) -> Result<Response<MergeEntitiesResponse>, Status> {
    let req = request.into_inner();
    let session_id = parse_uuid(&req.session_id)?;
    let merged = memory
        .entities
        .merge(session_id, &req.entity_name, &req.into)
        .await
        .map_err(status)?;
    Ok(Response::new(MergeEntitiesResponse { merged }))
}

pub async fn forget_update(
    memory: &Memory,
    request: Request<UpdateRefRequest>,
) -> Result<Response<UpdateRefResponse>, Status> {
    let req = request.into_inner();
    let (session_id, entry_id) = (parse_uuid(&req.session_id)?, parse_uuid(&req.entry_id)?);
    let existed = memory
        .records
        .forget(session_id, entry_id)
        .await
        .map_err(status)?;
    Ok(Response::new(UpdateRefResponse { existed }))
}

pub async fn purge_update(
    memory: &Memory,
    request: Request<UpdateRefRequest>,
) -> Result<Response<UpdateRefResponse>, Status> {
    let req = request.into_inner();
    let (session_id, entry_id) = (parse_uuid(&req.session_id)?, parse_uuid(&req.entry_id)?);
    let existed = memory
        .records
        .purge(session_id, entry_id)
        .await
        .map_err(status)?;
    Ok(Response::new(UpdateRefResponse { existed }))
}

pub async fn create_checkpoint(
    memory: &Memory,
    request: Request<CreateCheckpointRequest>,
) -> Result<Response<CreateCheckpointResponse>, Status> {
    let req = request.into_inner();
    debug!("gRPC CreateCheckpoint: session_id={}", req.session_id);

    let session_id = parse_uuid(&req.session_id)?;

    match memory.sessions.checkpoint(session_id).await {
        Ok(checkpoint_id) => Ok(Response::new(CreateCheckpointResponse {
            checkpoint_id: checkpoint_id.to_string(),
            success: true,
        })),
        Err(e @ post_cortex_memory::capabilities::SessionError::NotFound(_)) => Err(status(e)),
        Err(e) => {
            error!("gRPC CreateCheckpoint failed: {}", e);
            Err(status(e))
        }
    }
}

pub async fn query_context(
    memory: &Memory,
    request: Request<QueryContextRequest>,
) -> Result<Response<QueryContextResponse>, Status> {
    let req = request.into_inner();
    let session_id = parse_uuid(&req.session_id)?;
    // Wire names (`decision_made`, ...), comma-separated; an unknown
    // name is an invalid argument, never a match-all.
    let types = req
        .interaction_type
        .split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(|t| {
            t.parse::<UpdateType>()
                .map_err(|e| Status::invalid_argument(format!("interaction_type: {e}")))
        })
        .collect::<Result<Vec<_>, Status>>()?;
    let after =
        match req.after_unix {
            0 => None,
            t => Some(chrono::DateTime::from_timestamp(t, 0).ok_or_else(|| {
                Status::invalid_argument(format!("after_unix: {t} is out of range"))
            })?),
        };
    let page = memory
        .records
        .list(
            session_id,
            ListRecords {
                types,
                after,
                limit: req.limit as usize,
            },
        )
        .await
        .map_err(status)?;
    let updates = page
        .records
        .into_iter()
        .map(|(u, source)| ContextUpdateEntry {
            id: u.id.to_string(),
            interaction_type: u.update_type.as_str().to_string(),
            content: Some(ContextContent {
                title: u.content.title.clone(),
                description: u.content.description.clone(),
                details: u.content.details.clone(),
                examples: u.content.examples.clone(),
                implications: u.content.implications.clone(),
                code_ref: u.related_code.as_ref().map(|c| CodeReference {
                    file_path: c.file_path.clone(),
                    start_line: c.start_line,
                    end_line: c.end_line,
                    code_snippet: c.code_snippet.clone(),
                    commit_hash: c.commit_hash.clone().unwrap_or_default(),
                    branch: c.branch.clone().unwrap_or_default(),
                    change_description: c.change_description.clone(),
                }),
            }),
            timestamp_unix: u.timestamp.timestamp(),
            entities: u.creates_entities.clone(),
            source_ref: source.map(crate::memory::freshness::source_reference),
        })
        .collect();
    Ok(Response::new(QueryContextResponse {
        updates,
        total: page.total as u32,
    }))
}
