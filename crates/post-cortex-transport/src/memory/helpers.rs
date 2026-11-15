// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! Shared parsing / formatting / validation helpers used by the gRPC
//! service.
//!
//! After the single-entrypoint migration the write-path validation /
//! metadata-shaping helpers live in
//! [`post_cortex_memory::services::MemoryServiceImpl`]. This module is
//! left with: proto-UUID parsing, workspace formatting, and proto →
//! domain translation at the gRPC boundary (`proto_update_to_request`), and
//! the one mapping of each capability error onto a status.

use crate::grpc::status;
use post_cortex_core::core::context_update::{UpdateContent, UpdateType};
use post_cortex_core::services::UpdateContextRequest as ServiceUpdateRequest;
use post_cortex_core::workspace::SessionRole;
use tonic::{Request, Status};
use uuid::Uuid;

use post_cortex_proto::pb::{
    ContextUpdateItem as ProtoUpdateItem, UpdateContextRequest as ProtoUpdateRequest,
    WorkspaceInfo, WorkspaceSessionEntry,
};

pub fn parse_uuid(s: &str) -> Result<Uuid, Status> {
    Uuid::parse_str(s).map_err(|_| Status::invalid_argument(format!("Invalid UUID: {s}")))
}

pub fn get_session_id_from_metadata<T>(request: &Request<T>) -> Result<String, Status> {
    request
        .metadata()
        .get("x-session-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .ok_or_else(|| Status::invalid_argument("Missing x-session-id metadata"))
}

/// A session role of the wire: strict, never a default (the shared
/// parser of every transport).
pub fn parse_session_role(field: &str, s: &str) -> Result<SessionRole, Status> {
    post_cortex_memory::tools::wire::parse(field, s).map_err(status)
}

pub fn workspace_to_info(workspace: &post_cortex_core::workspace::Workspace) -> WorkspaceInfo {
    let created_at_unix = workspace
        .created_at
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    // The wire carries the stored role name ("Primary").
    let sessions: Vec<WorkspaceSessionEntry> = workspace
        .get_all_sessions()
        .into_iter()
        .map(|(session_id, role)| WorkspaceSessionEntry {
            session_id: session_id.to_string(),
            role: role.stored_name().to_string(),
        })
        .collect();

    let archived_at = workspace.archived_at();
    WorkspaceInfo {
        workspace_id: workspace.id.to_string(),
        name: workspace.name.clone(),
        slug: workspace.slug.clone(),
        description: workspace.description.clone(),
        created_at_unix,
        sessions,
        archived: archived_at.is_some(),
        archived_at_unix: archived_at.map_or(0, |t| t.timestamp()),
        remote_rerank: workspace.remote_rerank(),
    }
}

/// Translate the proto [`ContextContent.code_ref`] into the core
/// [`post_cortex_core::core::context_update::CodeReference`]. Returns
/// `None` when the proto field is missing.
fn proto_code_ref_to_domain(
    proto: Option<&post_cortex_proto::pb::CodeReference>,
) -> Option<post_cortex_core::core::context_update::CodeReference> {
    proto.map(|c| post_cortex_core::core::context_update::CodeReference {
        file_path: c.file_path.clone(),
        start_line: c.start_line,
        end_line: c.end_line,
        code_snippet: c.code_snippet.clone(),
        commit_hash: if c.commit_hash.is_empty() {
            None
        } else {
            Some(c.commit_hash.clone())
        },
        branch: if c.branch.is_empty() {
            None
        } else {
            Some(c.branch.clone())
        },
        change_description: c.change_description.clone(),
    })
}

/// Translate a proto [`UpdateContextRequest`] into the canonical
/// [`ServiceUpdateRequest`] consumed by
/// [`post_cortex_memory::services::MemoryServiceImpl::update_context`].
///
/// Performs the proto-only checks the canonical impl cannot do
/// (session-UUID parsing, `relation_type` enum lookup) and surfaces them
/// as `Status::invalid_argument`. Domain-level validation (referential
/// integrity, empty fields, self-relations) is left to the canonical
/// impl so every transport gets identical error messages.
pub fn proto_update_to_request(proto: ProtoUpdateRequest) -> Result<ServiceUpdateRequest, Status> {
    let session_id = parse_uuid(&proto.session_id)?;
    let item = ProtoUpdateItem {
        interaction_type: proto.interaction_type,
        content: proto.content,
        entities: proto.entities,
        relations: proto.relations,
        derived_from: proto.derived_from,
        valid_from: proto.valid_from,
        supersedes: proto.supersedes,
        confidence: proto.confidence,
    };
    proto_bulk_item_to_request(session_id, item, proto.author)
}

/// Translate a bulk `ContextUpdateItem` (which has no session_id of its own)
/// using the session_id from the bulk envelope.
pub fn proto_bulk_item_to_request(
    session_id: Uuid,
    item: ProtoUpdateItem,
    author: Option<String>,
) -> Result<ServiceUpdateRequest, Status> {
    let meta = proto_meta(item.valid_from, &item.supersedes, item.confidence)?;
    let interaction_type: UpdateType =
        post_cortex_memory::tools::wire::parse("interaction_type", &item.interaction_type)
            .map_err(status)?;
    let content_proto = item.content.unwrap_or_default();
    let derived_from = item.derived_from;
    let entities_proto = item.entities;
    let relations_proto = item.relations;
    let content = UpdateContent {
        title: content_proto.title.clone(),
        description: content_proto.description.clone(),
        details: content_proto.details.clone(),
        examples: content_proto.examples.clone(),
        implications: content_proto.implications.clone(),
    };

    let entities = post_cortex_memory::tools::wire::entities(
        entities_proto
            .iter()
            .map(|e| (e.name.as_str(), e.entity_type.as_str())),
        true,
        chrono::Utc::now(),
    )
    .map_err(status)?;
    let relations = post_cortex_memory::tools::wire::relations(relations_proto.iter().map(|r| {
        (
            r.from_entity.as_str(),
            r.to_entity.as_str(),
            r.relation_type.as_str(),
            r.context.as_str(),
        )
    }))
    .map_err(status)?;

    Ok(ServiceUpdateRequest {
        session_id,
        interaction_type,
        content,
        entities,
        relations,
        code_reference: proto_code_ref_to_domain(content_proto.code_ref.as_ref()),
        author: author
            .map(|a| a.trim().to_string())
            .filter(|a| !a.is_empty()),
        derived_from,
        meta,
    })
}

/// The writer's validity and trust statements of a proto update (the one
/// parser every transport shares).
fn proto_meta(
    valid_from: Option<String>,
    supersedes: &[String],
    confidence: Option<f32>,
) -> Result<post_cortex_core::core::context_update::UpdateMeta, Status> {
    post_cortex_memory::tools::wire::update_meta(valid_from.as_deref(), supersedes, confidence, "")
        .map_err(status)
}
