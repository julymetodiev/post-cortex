// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! gRPC context assembly over a session or a workspace.

use crate::grpc::status;
use crate::memory::capture;
use crate::memory::helpers::parse_uuid;
use tonic::{Request, Response, Status};
use tracing::debug;

use post_cortex_memory::Memory;
use post_cortex_proto::pb::{self, *};

pub async fn assemble_context(
    memory: &Memory,
    request: Request<AssembleContextRequest>,
) -> Result<Response<AssembleContextResponse>, Status> {
    let person = crate::person::of_metadata(request.metadata())?;
    let req = request.into_inner();
    debug!(
        "gRPC AssembleContext: session_id={}, workspace_id={:?}, query={}",
        req.session_id, req.workspace_id, req.query
    );

    use post_cortex_memory::capabilities::{AssembleRequest, Target};
    use post_cortex_memory::context_assembly;

    let corpus =
        post_cortex_memory::tools::wire::sources(req.sources.as_deref()).map_err(status)?;

    // Workspace scope takes precedence over the session.
    let target = match req.workspace_id.as_deref().filter(|s| !s.is_empty()) {
        Some(raw) => Target::Workspace(parse_uuid(raw)?),
        None => Target::Session(parse_uuid(&req.session_id)?),
    };
    let assembled = memory
        .assembly
        .assemble(AssembleRequest {
            query: req.query.clone(),
            target,
            token_budget: req.token_budget as usize,
            corpus,
            person,
        })
        .await
        .map_err(status)?;

    let formatted_text = context_assembly::format_for_llm(&assembled);

    let items: Vec<AssembledContextItem> = assembled
        .items
        .iter()
        .map(|item| {
            let source = match &item.source {
                context_assembly::ContextSource::SemanticMatch => "semantic_match",
                context_assembly::ContextSource::Conversation => "conversation",
            };
            AssembledContextItem {
                text: item.text.clone(),
                score: item.score,
                source: source.to_string(),
                entities: item.entities.clone(),
                token_estimate: item.token_estimate as u32,
                entry_id: item.entry_id.clone(),
            }
        })
        .collect();

    let entity_context: Vec<AssembledEntityContext> = assembled
        .entity_context
        .iter()
        .map(|ec| {
            let (relevance, via_entity, via_relation) = match &ec.relevance {
                context_assembly::EntityRelevance::DirectMention => {
                    ("direct_mention".to_string(), String::new(), String::new())
                }
                context_assembly::EntityRelevance::GraphNeighbor { via, relation } => {
                    ("graph_neighbor".to_string(), via.clone(), relation.clone())
                }
            };
            let relationships: Vec<EntityRelation> = ec
                .relationships
                .iter()
                .map(|r| EntityRelation {
                    from_entity: r.from_entity.clone(),
                    to_entity: r.to_entity.clone(),
                    relation_type: r.relation_type.as_str().to_string(),
                    context: r.context.clone(),
                })
                .collect();
            AssembledEntityContext {
                name: ec.name.clone(),
                relevance,
                via_entity,
                via_relation,
                relationships,
            }
        })
        .collect();

    let impact: Vec<pb::ImpactEntry> = assembled
        .impact
        .iter()
        .map(|i| pb::ImpactEntry {
            entity: i.entity.clone(),
            depends_on: i.depends_on.clone(),
            relation_type: i.relation_type.as_str().to_string(),
            context: i.context.clone(),
        })
        .collect();

    Ok(Response::new(AssembleContextResponse {
        items,
        entity_context,
        impact,
        total_tokens: assembled.total_tokens as u32,
        formatted_text,
        cards: assembled.cards.iter().map(capture::card_info).collect(),
    }))
}
