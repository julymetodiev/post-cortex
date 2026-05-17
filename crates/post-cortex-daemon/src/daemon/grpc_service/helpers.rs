// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! Shared parsing / formatting / validation helpers used by the gRPC
//! service.
//!
//! After the single-entrypoint migration the write-path validation /
//! metadata-shaping helpers live in
//! [`post_cortex_memory::services::MemoryServiceImpl`]. This module is
//! left with: proto-UUID parsing, workspace formatting, and proto →
//! domain translation at the gRPC boundary (`proto_update_to_request`,
//! `system_error_to_status`).

use post_cortex_core::core::context_update::{
    EntityData, EntityRelationship, EntityType, RelationType, UpdateContent, UpdateType,
};
use post_cortex_core::core::error::SystemError;
use post_cortex_core::services::UpdateContextRequest as ServiceUpdateRequest;
use post_cortex_core::workspace::SessionRole;
use tonic::{Request, Status};
use uuid::Uuid;

use super::pb::{
    ContextUpdateItem as ProtoUpdateItem, UpdateContextRequest as ProtoUpdateRequest,
    WorkspaceInfo, WorkspaceSessionEntry,
};

pub(super) fn parse_uuid(s: &str) -> Result<Uuid, Status> {
    Uuid::parse_str(s).map_err(|_| Status::invalid_argument(format!("Invalid UUID: {s}")))
}

pub(super) fn get_session_id_from_metadata<T>(request: &Request<T>) -> Result<String, Status> {
    request
        .metadata()
        .get("x-session-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .ok_or_else(|| Status::unauthenticated("Missing x-session-id metadata"))
}

pub(super) fn parse_session_role(s: &str) -> SessionRole {
    match s.to_lowercase().as_str() {
        "primary" => SessionRole::Primary,
        "dependency" => SessionRole::Dependency,
        "shared" => SessionRole::Shared,
        _ => SessionRole::Related,
    }
}

pub(super) fn workspace_to_info(workspace: &post_cortex_core::workspace::Workspace) -> WorkspaceInfo {
    let created_at_unix = workspace
        .created_at
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    let sessions: Vec<WorkspaceSessionEntry> = workspace
        .session_ids
        .iter()
        .map(|entry| WorkspaceSessionEntry {
            session_id: entry.key().to_string(),
            role: format!("{:?}", entry.value()),
        })
        .collect();

    WorkspaceInfo {
        workspace_id: workspace.id.to_string(),
        name: workspace.name.clone(),
        description: workspace.description.clone(),
        created_at_unix,
        sessions,
    }
}

/// Parse a relation_type string (case-insensitive) into a [`RelationType`],
/// returning `None` for unknown values.
fn parse_relation_type(s: &str) -> Option<RelationType> {
    match s.to_lowercase().as_str() {
        "required_by" | "requiredby" => Some(RelationType::RequiredBy),
        "leads_to" | "leadsto" => Some(RelationType::LeadsTo),
        "related_to" | "relatedto" => Some(RelationType::RelatedTo),
        "conflicts_with" | "conflictswith" => Some(RelationType::ConflictsWith),
        "depends_on" | "dependson" => Some(RelationType::DependsOn),
        "implements" => Some(RelationType::Implements),
        "caused_by" | "causedby" => Some(RelationType::CausedBy),
        "solves" => Some(RelationType::Solves),
        _ => None,
    }
}

/// Parse an entity_type string (case-insensitive) into an [`EntityType`],
/// defaulting to `Concept`.
fn parse_entity_type(s: &str) -> EntityType {
    match s.to_lowercase().as_str() {
        "technology" => EntityType::Technology,
        "concept" => EntityType::Concept,
        "problem" => EntityType::Problem,
        "solution" => EntityType::Solution,
        "decision" => EntityType::Decision,
        "code_component" | "codecomponent" => EntityType::CodeComponent,
        _ => EntityType::Concept,
    }
}

/// Parse a proto `interaction_type` string into a typed [`UpdateType`].
/// Unknown values fall back to `ConceptDefined` to match historical
/// behaviour (the legacy `build_update_metadata` used the same catch-all).
fn parse_update_type(s: &str) -> UpdateType {
    match s {
        "decision_made" => UpdateType::DecisionMade,
        "problem_solved" => UpdateType::ProblemSolved,
        "code_change" | "code_changed" => UpdateType::CodeChanged,
        "qa" | "question_answered" => UpdateType::QuestionAnswered,
        "requirement_added" => UpdateType::RequirementAdded,
        _ => UpdateType::ConceptDefined,
    }
}

/// Translate the proto [`ContextContent.code_ref`] into the core
/// [`post_cortex_core::core::context_update::CodeReference`]. Returns
/// `None` when the proto field is missing.
fn proto_code_ref_to_domain(
    proto: Option<&super::pb::CodeReference>,
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
pub(super) fn proto_update_to_request(
    proto: ProtoUpdateRequest,
) -> Result<ServiceUpdateRequest, Status> {
    let session_id = parse_uuid(&proto.session_id)?;
    translate_payload(
        session_id,
        &proto.interaction_type,
        proto.content,
        &proto.entities,
        &proto.relations,
    )
}

/// Translate a bulk `ContextUpdateItem` (which has no session_id of its own)
/// using the session_id from the bulk envelope.
pub(super) fn proto_bulk_item_to_request(
    session_id: Uuid,
    item: ProtoUpdateItem,
) -> Result<ServiceUpdateRequest, Status> {
    translate_payload(
        session_id,
        &item.interaction_type,
        item.content,
        &item.entities,
        &item.relations,
    )
}

fn translate_payload(
    session_id: Uuid,
    interaction_type: &str,
    content_proto: Option<super::pb::ContextContent>,
    entities_proto: &[super::pb::EntityInfo],
    relations_proto: &[super::pb::RelationInfo],
) -> Result<ServiceUpdateRequest, Status> {
    let interaction_type = parse_update_type(interaction_type);
    let content_proto = content_proto.unwrap_or_default();
    let content = UpdateContent {
        title: content_proto.title.clone(),
        description: content_proto.description.clone(),
        details: content_proto.details.clone(),
        examples: content_proto.examples.clone(),
        implications: content_proto.implications.clone(),
    };

    let entities: Vec<EntityData> = entities_proto
        .iter()
        .map(|e| EntityData {
            name: e.name.clone(),
            entity_type: parse_entity_type(&e.entity_type),
            first_mentioned: chrono::Utc::now(),
            last_mentioned: chrono::Utc::now(),
            mention_count: 1,
            importance_score: 1.0,
            description: None,
        })
        .collect();

    let mut relations = Vec::with_capacity(relations_proto.len());
    for (i, r) in relations_proto.iter().enumerate() {
        let rt = parse_relation_type(&r.relation_type).ok_or_else(|| {
            Status::invalid_argument(format!(
                "relation[{i}]: unknown relation_type {:?}; valid values are: \
                 required_by, leads_to, related_to, conflicts_with, depends_on, implements, caused_by, solves",
                r.relation_type
            ))
        })?;
        relations.push(EntityRelationship {
            from_entity: r.from_entity.clone(),
            to_entity: r.to_entity.clone(),
            relation_type: rt,
            context: r.context.clone(),
        });
    }

    Ok(ServiceUpdateRequest {
        session_id,
        interaction_type,
        content,
        entities,
        relations,
        code_reference: proto_code_ref_to_domain(content_proto.code_ref.as_ref()),
    })
}

/// Map a domain [`SystemError`] onto the appropriate tonic [`Status`].
/// Validation failures surface as `invalid_argument`; everything else is
/// `internal`. Storage / circuit-breaker errors are deliberately not
/// promoted to `unavailable` here — the existing handlers always
/// returned `internal` for those, and changing that is out of scope for
/// the single-entrypoint migration.
pub(super) fn system_error_to_status(err: SystemError) -> Status {
    match err {
        SystemError::InvalidArgument(msg) => Status::invalid_argument(msg),
        SystemError::SessionNotFound(id) => Status::not_found(format!("session {id} not found")),
        SystemError::WorkspaceNotFound(id) => {
            Status::not_found(format!("workspace {id} not found"))
        }
        other => Status::internal(other.to_string()),
    }
}
