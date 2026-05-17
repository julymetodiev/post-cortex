// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! Shared parsing / formatting / validation helpers used by the gRPC
//! service.

use crate::workspace::SessionRole;
use tonic::{Request, Status};
use uuid::Uuid;

use super::pb::{ContextContent, EntityInfo, RelationInfo, WorkspaceInfo, WorkspaceSessionEntry};

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

pub(super) fn workspace_to_info(workspace: &crate::workspace::Workspace) -> WorkspaceInfo {
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

pub(super) fn format_context_description(
    interaction_type: &str,
    content: &ContextContent,
) -> String {
    let mut desc = String::new();
    if !content.title.is_empty() {
        desc.push_str(&content.title);
    }
    if !content.description.is_empty() {
        if !desc.is_empty() {
            desc.push_str(": ");
        }
        desc.push_str(&content.description);
    }
    if desc.is_empty() {
        desc = format!("[{interaction_type}] update");
    }
    desc
}

/// Validate entities and relations provided by the caller (Claude-driven extraction).
/// Returns `Err(Status)` with a precise message on the first validation failure.
pub(super) fn validate_entities_and_relations(
    entities: &[EntityInfo],
    relations: &[RelationInfo],
) -> Result<(), Status> {
    use std::collections::HashSet;

    if entities.is_empty() {
        return Err(Status::invalid_argument(
            "entities field is required and must not be empty",
        ));
    }

    if relations.is_empty() {
        return Err(Status::invalid_argument(
            "relations field is required and must not be empty",
        ));
    }

    // Build a set of known entity names for referential-integrity checks.
    let entity_names: HashSet<&str> = entities.iter().map(|e| e.name.as_str()).collect();

    for (i, rel) in relations.iter().enumerate() {
        if rel.from_entity.is_empty() {
            return Err(Status::invalid_argument(format!(
                "relation[{i}]: from_entity must not be empty"
            )));
        }
        if rel.to_entity.is_empty() {
            return Err(Status::invalid_argument(format!(
                "relation[{i}]: to_entity must not be empty"
            )));
        }
        if rel.from_entity == rel.to_entity {
            return Err(Status::invalid_argument(format!(
                "relation[{i}]: self-relations are not allowed (from_entity == to_entity == {:?})",
                rel.from_entity
            )));
        }
        if rel.context.is_empty() {
            return Err(Status::invalid_argument(format!(
                "relation[{i}]: context must not be empty — every relation requires an explanation"
            )));
        }
        // Validate relation_type is a known variant.
        if parse_relation_type(&rel.relation_type).is_none() {
            return Err(Status::invalid_argument(format!(
                "relation[{i}]: unknown relation_type {:?}; valid values are: \
                 required_by, leads_to, related_to, conflicts_with, depends_on, implements, caused_by, solves",
                rel.relation_type
            )));
        }
        // Referential integrity: both endpoints must be declared in entities.
        if !entity_names.contains(rel.from_entity.as_str()) {
            return Err(Status::invalid_argument(format!(
                "relation[{i}]: from_entity {:?} is not declared in the entities list",
                rel.from_entity
            )));
        }
        if !entity_names.contains(rel.to_entity.as_str()) {
            return Err(Status::invalid_argument(format!(
                "relation[{i}]: to_entity {:?} is not declared in the entities list",
                rel.to_entity
            )));
        }
    }

    Ok(())
}

/// Parse a relation_type string (case-insensitive) into a `RelationType`,
/// returning `None` for unknown values.
pub(super) fn parse_relation_type(s: &str) -> Option<crate::core::context_update::RelationType> {
    use crate::core::context_update::RelationType;
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

/// Parse an entity_type string (case-insensitive) into an `EntityType`,
/// defaulting to `Concept`.
pub(super) fn parse_entity_type(s: &str) -> crate::core::context_update::EntityType {
    use crate::core::context_update::EntityType;
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

pub(super) fn build_update_metadata(
    interaction_type: &str,
    content: &ContextContent,
    entities: &[EntityInfo],
    relations: &[RelationInfo],
) -> serde_json::Value {
    use crate::core::context_update::{
        ContextUpdate, EntityRelationship, TypedEntity, UpdateContent, UpdateType,
    };

    let update_type = match interaction_type {
        "decision_made" => UpdateType::DecisionMade,
        "problem_solved" => UpdateType::ProblemSolved,
        "code_change" | "code_changed" => UpdateType::CodeChanged,
        "qa" | "question_answered" => UpdateType::QuestionAnswered,
        "requirement_added" => UpdateType::RequirementAdded,
        "concept_defined" | _ => UpdateType::ConceptDefined,
    };

    let related_code = content
        .code_ref
        .as_ref()
        .map(|c| crate::core::context_update::CodeReference {
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
        });

    // Map proto EntityInfo → TypedEntity and collect entity names for creates_entities.
    let typed_entities: Vec<TypedEntity> = entities
        .iter()
        .map(|e| TypedEntity {
            name: e.name.clone(),
            entity_type: parse_entity_type(&e.entity_type),
        })
        .collect();
    let creates_entities: Vec<String> = typed_entities.iter().map(|e| e.name.clone()).collect();

    // Map proto RelationInfo → EntityRelationship.
    let creates_relationships: Vec<EntityRelationship> = relations
        .iter()
        .filter_map(|r| {
            parse_relation_type(&r.relation_type).map(|rt| EntityRelationship {
                from_entity: r.from_entity.clone(),
                to_entity: r.to_entity.clone(),
                relation_type: rt,
                context: r.context.clone(),
            })
        })
        .collect();

    let update = ContextUpdate {
        id: Uuid::new_v4(),
        timestamp: chrono::Utc::now(),
        update_type,
        content: UpdateContent {
            title: content.title.clone(),
            description: content.description.clone(),
            details: content.details.clone(),
            examples: content.examples.clone(),
            implications: content.implications.clone(),
        },
        related_code,
        parent_update: None,
        user_marked_important: false,
        creates_entities,
        creates_relationships,
        references_entities: Vec::new(),
        typed_entities,
    };

    serde_json::to_value(update).expect("ContextUpdate serialization cannot fail")
}
