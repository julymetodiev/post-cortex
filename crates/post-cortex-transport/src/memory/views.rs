// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! gRPC graph views, summaries, key decisions and insights, entity
//! importance and networks, session statistics.

use crate::grpc::status;
use crate::memory::analysis;
use crate::memory::helpers::parse_uuid;
use post_cortex_memory::capabilities::SummaryRequest;
use tonic::{Request, Response, Status};
use tracing::debug;
use uuid::Uuid;

use post_cortex_memory::Memory;
use post_cortex_proto::pb::*;

pub async fn get_graph_view(
    memory: &Memory,
    request: Request<GetGraphViewRequest>,
) -> Result<Response<GraphView>, Status> {
    use post_cortex_memory::capabilities::Target;
    use post_cortex_memory::graph_views::ViewRequest;

    let req = request.into_inner();
    let optional = |raw: &str| -> Result<Option<Uuid>, Status> {
        if raw.is_empty() {
            Ok(None)
        } else {
            parse_uuid(raw).map(Some)
        }
    };
    let (session_id, workspace_id) = (optional(&req.session_id)?, optional(&req.workspace_id)?);
    // A limit or depth of 0 is not given.
    let given = |n: u32| (n != 0).then_some(n as usize);
    let view = match req.view {
        Some(get_graph_view_request::View::Entity(p)) => {
            ViewRequest::entity(&p.entity, given(p.limit))
        }
        Some(get_graph_view_request::View::Impact(p)) => {
            ViewRequest::impact(&p.entity, given(p.depth), given(p.limit))
        }
        Some(get_graph_view_request::View::Topics(p)) => {
            Ok(ViewRequest::topics(given(p.limit), given(p.per_topic)))
        }
        Some(get_graph_view_request::View::Lineage(p)) => {
            ViewRequest::lineage(&p.entity, given(p.depth))
        }
        Some(get_graph_view_request::View::Radar(p)) => {
            Ok(ViewRequest::radar(given(p.limit), p.include_dismissed))
        }
        Some(get_graph_view_request::View::File(p)) => {
            ViewRequest::file(&p.path, given(p.depth), given(p.limit))
        }
        None => Err("the request names no view".to_string()),
    }
    .map_err(|e| status(post_cortex_memory::tools::Failure::invalid(e)))?;
    let target = Target::of(session_id, workspace_id).map_err(status)?;
    let rendered = memory.views.view(target, view).await.map_err(status)?;
    Ok(Response::new(analysis::graph_view(rendered)))
}

pub async fn get_structured_summary(
    memory: &Memory,
    request: Request<GetStructuredSummaryRequest>,
) -> Result<Response<StructuredSummary>, Status> {
    let req = request.into_inner();
    debug!("gRPC GetStructuredSummary: session_id={}", req.session_id);

    let session_id = parse_uuid(&req.session_id)?;
    let sections: Vec<_> = req.include().filter_map(analysis::section).collect();
    let sections = (!sections.is_empty()).then_some(sections);
    // The one summary operation (MCP get_structured_summary uses it too).
    let summary = memory
        .views
        .summary(SummaryRequest {
            session_id,
            decisions_limit: req.decisions_limit.map(|v| v as usize),
            entities_limit: req.entities_limit.map(|v| v as usize),
            questions_limit: req.questions_limit.map(|v| v as usize),
            concepts_limit: req.concepts_limit.map(|v| v as usize),
            min_confidence: req.min_confidence,
            compact: req.compact.unwrap_or(false),
            sections,
            insights_limit: req.insights_limit.map(|v| v as usize),
        })
        .await
        .map_err(status)?;
    Ok(Response::new(analysis::summary(summary)))
}

pub async fn get_key_decisions(
    memory: &Memory,
    request: Request<GetKeyDecisionsRequest>,
) -> Result<Response<KeyDecisions>, Status> {
    let req = request.into_inner();
    debug!("gRPC GetKeyDecisions: session_id={}", req.session_id);

    let session_id = parse_uuid(&req.session_id)?;
    let decisions = memory
        .views
        .key_decisions(session_id)
        .await
        .map_err(status)?;
    Ok(Response::new(analysis::decisions(decisions)))
}

pub async fn get_key_insights(
    memory: &Memory,
    request: Request<GetKeyInsightsRequest>,
) -> Result<Response<KeyInsights>, Status> {
    let req = request.into_inner();
    debug!("gRPC GetKeyInsights: session_id={}", req.session_id);

    let session_id = parse_uuid(&req.session_id)?;
    let insights = memory
        .views
        .key_insights(session_id, req.limit.map(|v| v as usize))
        .await
        .map_err(status)?;
    Ok(Response::new(KeyInsights { insights }))
}

pub async fn get_entity_importance(
    memory: &Memory,
    request: Request<GetEntityImportanceRequest>,
) -> Result<Response<EntityImportance>, Status> {
    let req = request.into_inner();
    debug!("gRPC GetEntityImportance: session_id={}", req.session_id);
    let session_id = parse_uuid(&req.session_id)?;
    // The one operation and shape of MCP `entity_importance`.
    let importance = memory
        .views
        .entity_importance(
            session_id,
            req.limit.map(|l| l as usize),
            req.min_importance,
        )
        .await
        .map_err(status)?;
    Ok(Response::new(analysis::importance(importance)))
}

pub async fn get_entity_network(
    memory: &Memory,
    request: Request<GetEntityNetworkRequest>,
) -> Result<Response<EntityNetwork>, Status> {
    let req = request.into_inner();
    debug!("gRPC GetEntityNetwork: session_id={}", req.session_id);
    let session_id = parse_uuid(&req.session_id)?;
    let network = memory
        .views
        .entity_network(
            session_id,
            req.center_entity.as_deref(),
            req.max_entities.map(|n| n as usize),
            req.max_relationships.map(|n| n as usize),
        )
        .await
        .map_err(status)?;
    Ok(Response::new(analysis::network(network)))
}

pub async fn get_session_statistics(
    memory: &Memory,
    request: Request<GetSessionStatisticsRequest>,
) -> Result<Response<SessionStatistics>, Status> {
    let req = request.into_inner();
    debug!("gRPC GetSessionStatistics: session_id={}", req.session_id);
    let session_id = parse_uuid(&req.session_id)?;
    let stats = memory.views.statistics(session_id).await.map_err(status)?;
    Ok(Response::new(analysis::statistics(stats)))
}
