// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! gRPC search methods: semantic search, related content, vectorization.

use crate::grpc::status;
use crate::memory::helpers::parse_uuid;
use post_cortex_memory::capabilities::SearchScope;
use post_cortex_memory::content_vectorizer::SearchOptions;
use tonic::{Request, Response, Status};
use tracing::debug;
use tracing::error;

use post_cortex_memory::Memory;
use post_cortex_proto::pb::*;

pub async fn semantic_search(
    memory: &Memory,
    request: Request<SemanticSearchRequest>,
) -> Result<Response<SemanticSearchResponse>, Status> {
    let req = request.into_inner();
    debug!(
        "gRPC SemanticSearch: query='{}', session='{}', workspace='{:?}'",
        req.query, req.session_id, req.workspace_id
    );

    let max_results = if req.max_results > 0 {
        req.max_results as usize
    } else {
        10
    };

    {
        let mode = post_cortex_memory::tools::wire::mode(req.mode.as_deref()).map_err(status)?;
        let corpus =
            post_cortex_memory::tools::wire::sources(req.sources.as_deref()).map_err(status)?;
        let options = SearchOptions {
            limit: Some(max_results),
            mode,
            corpus,
            author: req.author.clone().filter(|a| !a.trim().is_empty()),
            prefer_author: req.prefer_author.clone().filter(|a| !a.trim().is_empty()),
            trace: req.trace.unwrap_or(false),
            ..Default::default()
        };
        // workspace_id takes precedence over session_id
        let workspace_id_str = req.workspace_id.as_deref().unwrap_or("");
        let scope = if !workspace_id_str.is_empty() {
            SearchScope::Workspace(parse_uuid(workspace_id_str)?)
        } else if req.session_id.is_empty() {
            SearchScope::Global
        } else {
            SearchScope::Session(parse_uuid(&req.session_id)?)
        };
        let found = memory
            .search
            .find(scope, &req.query, options)
            .await
            .map_err(status)?;

        let min_score = if req.min_score > 0.0 {
            req.min_score
        } else {
            0.0
        };

        let search_results: Vec<_> = found
            .hits
            .iter()
            .filter(|r| r.combined_score >= min_score)
            .cloned()
            .collect();
        let total_matches = search_results.len() as u32;

        let results: Vec<SearchResult> = search_results.into_iter().map(search_result).collect();

        Ok(Response::new(SemanticSearchResponse {
            results,
            total_matches,
            reranked: found.reranked,
        }))
    }
}

pub async fn find_related_content(
    memory: &Memory,
    request: Request<FindRelatedContentRequest>,
) -> Result<Response<FindRelatedContentResponse>, Status> {
    let req = request.into_inner();
    debug!(
        "gRPC FindRelatedContent: session={}, topic='{}'",
        req.session_id, req.topic
    );

    if req.topic.is_empty() {
        return Err(Status::invalid_argument("topic cannot be empty"));
    }

    {
        let session_id = parse_uuid(&req.session_id)?;
        let max_results = req.limit.unwrap_or(10) as usize;

        let search_results = memory
            .search
            .search(
                SearchScope::Session(session_id),
                &req.topic,
                SearchOptions {
                    limit: Some(max_results),
                    ..Default::default()
                },
            )
            .await
            .map_err(status)?;

        let results: Vec<SearchResult> = search_results.into_iter().map(search_result).collect();

        Ok(Response::new(FindRelatedContentResponse { results }))
    }
}

pub async fn vectorize_session(
    memory: &Memory,
    request: Request<VectorizeSessionRequest>,
) -> Result<Response<VectorizeSessionResponse>, Status> {
    let req = request.into_inner();
    debug!("gRPC VectorizeSession: session_id={}", req.session_id);

    let session_id = parse_uuid(&req.session_id)?;

    {
        match memory.admin.vectorize_session(session_id).await {
            Ok(vectors_created) => Ok(Response::new(VectorizeSessionResponse {
                success: true,
                vectors_created: vectors_created as u32,
            })),
            Err(e) => {
                error!("gRPC VectorizeSession failed: {}", e);
                Err(status(e))
            }
        }
    }
}

pub async fn get_vectorization_stats(
    memory: &Memory,
    _request: Request<GetVectorizationStatsRequest>,
) -> Result<Response<VectorizationStats>, Status> {
    debug!("gRPC GetVectorizationStats");
    let stats = memory.admin.vectorization_stats().await.map_err(|e| {
        error!("gRPC GetVectorizationStats failed: {}", e);
        status(e)
    })?;
    Ok(Response::new(crate::memory::analysis::vectorization(stats)))
}

/// A hit on the wire.
fn search_result(r: post_cortex_memory::content_vectorizer::SemanticSearchResult) -> SearchResult {
    SearchResult {
        content: r.text_content,
        score: r.combined_score,
        session_id: r
            .source
            .session()
            .map(|id| id.to_string())
            .unwrap_or_default(),
        workspace_id: r
            .source
            .workspace()
            .map(|id| id.to_string())
            .unwrap_or_default(),
        content_type: r.content_type.as_str().to_string(),
        entry_id: r.content_id,
        neighbour_of: r.neighbour_of,
        author: r.author,
        title: r.title,
        derived_from: r.derived_from,
        validity: r.validity.map(crate::views::response::validity),
        confidence: r.confidence,
        signals: r.signals.map(|s| SearchSignals {
            vector_rank: s.vector_rank.map(|n| n as u64),
            vector_similarity: s.vector_similarity,
            bm25_rank: s.bm25_rank.map(|n| n as u64),
            graph_rank: s.graph_rank.map(|n| n as u64),
            fused: s.fused,
            rerank: s.rerank,
        }),
        document: r.document.map(|d| DocumentHit {
            kind: d.kind,
            version: d.version,
        }),
    }
}
