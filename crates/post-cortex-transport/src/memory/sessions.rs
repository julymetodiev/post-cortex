// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! gRPC session methods: create, list, load, search, rename, delete.

use crate::grpc::status;
use crate::memory::helpers::parse_uuid;
use tonic::{Request, Response, Status};
use tracing::{debug, error};

use post_cortex_memory::Memory;
use post_cortex_proto::pb::*;

pub async fn create_session(
    memory: &Memory,
    request: Request<CreateSessionRequest>,
) -> Result<Response<CreateSessionResponse>, Status> {
    let req = request.into_inner();
    debug!("gRPC CreateSession: name={}", req.name);

    let name = if req.name.is_empty() {
        None
    } else {
        Some(req.name)
    };
    let description = if req.description.is_empty() {
        None
    } else {
        Some(req.description)
    };

    match memory.sessions.create(name, description).await {
        Ok(session_id) => Ok(Response::new(CreateSessionResponse {
            session_id: session_id.to_string(),
        })),
        Err(e) => {
            error!("gRPC CreateSession failed: {}", e);
            Err(status(e))
        }
    }
}

pub async fn list_sessions(
    memory: &Memory,
    request: Request<ListSessionsRequest>,
) -> Result<Response<ListSessionsResponse>, Status> {
    let req = request.into_inner();

    // Metadata records only: listing never loads sessions.
    let summaries = if !req.name_filter.is_empty() {
        memory
            .sessions
            .find(&req.name_filter)
            .await
            .map_err(status)?
    } else {
        memory.sessions.list_summaries().await.map_err(status)?
    };

    let limit = if req.limit > 0 {
        req.limit as usize
    } else {
        100
    };

    let sessions = summaries
        .into_iter()
        .take(limit)
        .map(session_info)
        .collect();

    Ok(Response::new(ListSessionsResponse { sessions }))
}

pub async fn load_session(
    memory: &Memory,
    request: Request<LoadSessionRequest>,
) -> Result<Response<LoadSessionResponse>, Status> {
    let req = request.into_inner();
    debug!("gRPC LoadSession: session_id={}", req.session_id);

    let session_id = parse_uuid(&req.session_id)?;

    let session = memory
        .sessions
        .get(session_id)
        .await
        .map_err(|e| Status::not_found(e.to_string()))?;

    let info = SessionInfo {
        session_id: session_id.to_string(),
        name: session.name().unwrap_or_default(),
        description: session.description().unwrap_or_default(),
        created_at_unix: session.created_at().timestamp(),
        update_count: session.records().len() as u32,
    };

    Ok(Response::new(LoadSessionResponse {
        session: Some(info),
    }))
}

pub async fn search_sessions(
    memory: &Memory,
    request: Request<SearchSessionsRequest>,
) -> Result<Response<SearchSessionsResponse>, Status> {
    let req = request.into_inner();
    debug!("gRPC SearchSessions: query={}", req.query);

    let sessions = memory
        .sessions
        .find(&req.query)
        .await
        .map_err(status)?
        .into_iter()
        .map(session_info)
        .collect();

    Ok(Response::new(SearchSessionsResponse { sessions }))
}

pub async fn update_session_metadata(
    memory: &Memory,
    request: Request<UpdateSessionMetadataRequest>,
) -> Result<Response<UpdateSessionMetadataResponse>, Status> {
    let req = request.into_inner();
    debug!("gRPC UpdateSessionMetadata: session_id={}", req.session_id);

    let session_id = parse_uuid(&req.session_id)?;

    match memory
        .sessions
        .update_metadata(session_id, req.name, req.description)
        .await
    {
        Ok(()) => Ok(Response::new(UpdateSessionMetadataResponse {
            success: true,
        })),
        Err(e) => {
            error!("gRPC UpdateSessionMetadata failed: {}", e);
            Err(status(e))
        }
    }
}

pub async fn delete_session(
    memory: &Memory,
    request: Request<DeleteSessionRequest>,
) -> Result<Response<DeleteSessionResponse>, Status> {
    let req = request.into_inner();
    debug!("gRPC DeleteSession: session_id={}", req.session_id);

    let session_id = parse_uuid(&req.session_id)?;

    match memory.sessions.delete(session_id).await {
        Ok(success) => Ok(Response::new(DeleteSessionResponse { success })),
        Err(e) => {
            error!("gRPC DeleteSession failed: {}", e);
            Err(status(e))
        }
    }
}

/// Proto listing row for a session.
pub fn session_info(summary: post_cortex_ports::SessionSummary) -> SessionInfo {
    SessionInfo {
        session_id: summary.id.to_string(),
        name: summary.name.unwrap_or_default(),
        description: summary.description.unwrap_or_default(),
        created_at_unix: summary.created_at.timestamp(),
        update_count: summary.update_count as u32,
    }
}
