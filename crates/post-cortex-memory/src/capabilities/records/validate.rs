// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The one validation of a write, and the record a valid write becomes.

use std::collections::HashSet;

use chrono::Utc;
use post_cortex_core::core::context_update::{ContextUpdate, TypedEntity};
use post_cortex_core::core::message;
use post_cortex_core::redact::RedactionCounts;
use post_cortex_core::services::UpdateContextRequest;
use post_cortex_core::session::active_session::ActiveSession;
use uuid::Uuid;

use crate::capabilities::records::RecordError;

fn invalid(msg: impl Into<String>) -> RecordError {
    RecordError::InvalidArgument(msg.into())
}

/// Redaction never fails a write; it is logged so a spike (e.g. an agent
/// pasting a log full of addresses) is visible without exposing the values.
pub fn log_redactions(session_id: Uuid, counts: &RedactionCounts) {
    if !counts.is_empty() {
        tracing::info!(%session_id, ?counts, "update_context: redacted sensitive values");
    }
}

/// Rejects a write that would corrupt the entity graph: empty title and
/// description, missing entities or relations, dangling relation endpoints,
/// self-relations, empty relation context.
pub fn request(req: &UpdateContextRequest) -> Result<(), RecordError> {
    if req.content.title.trim().is_empty() && req.content.description.trim().is_empty() {
        return Err(invalid(
            "update_context: title and description are both empty — give at least one, by the content keys of its interaction type (decision_made: decision, rationale; code_change: file, changes; ...) or as title, description",
        ));
    }
    if req.entities.is_empty() {
        return Err(invalid("update_context: entities must not be empty"));
    }
    if req.relations.is_empty() {
        return Err(invalid("update_context: relations must not be empty"));
    }

    let entity_names: HashSet<&str> = req.entities.iter().map(|e| e.name.as_str()).collect();
    for (i, rel) in req.relations.iter().enumerate() {
        if rel.from_entity.is_empty() {
            return Err(invalid(format!(
                "relation[{i}]: from_entity must not be empty"
            )));
        }
        if rel.to_entity.is_empty() {
            return Err(invalid(format!(
                "relation[{i}]: to_entity must not be empty"
            )));
        }
        if rel.from_entity == rel.to_entity {
            return Err(invalid(format!(
                "relation[{i}]: self-relations are not allowed (from_entity == to_entity == {:?})",
                rel.from_entity
            )));
        }
        if rel.context.trim().is_empty() {
            return Err(invalid(format!(
                "relation[{i}]: context must not be empty — every relation requires an explanation"
            )));
        }
        if !entity_names.contains(rel.from_entity.as_str()) {
            return Err(invalid(format!(
                "relation[{i}]: from_entity {:?} is not declared in the entities list",
                rel.from_entity
            )));
        }
        if !entity_names.contains(rel.to_entity.as_str()) {
            return Err(invalid(format!(
                "relation[{i}]: to_entity {:?} is not declared in the entities list",
                rel.to_entity
            )));
        }
    }

    Ok(())
}

/// `meta` is well-formed: confidence in [0, 1], no `retired_at`, and every
/// superseded record exists in the session.
pub fn meta(req: &UpdateContextRequest, session: &ActiveSession) -> Result<(), RecordError> {
    if let Some(c) = req.meta.confidence
        && !(0.0..=1.0).contains(&c)
    {
        return Err(invalid(format!(
            "confidence must be between 0 and 1, got {c}"
        )));
    }
    if req.meta.retired_at.is_some() {
        return Err(invalid("retired_at is set by forget, not on write"));
    }
    if req.meta.supersedes.is_empty() {
        return Ok(());
    }
    let known: HashSet<Uuid> = session.records().iter().map(|u| u.id).collect();
    match req.meta.supersedes.iter().find(|id| !known.contains(id)) {
        Some(missing) => Err(invalid(format!(
            "supersedes: entry {missing} is not in session {}",
            req.session_id
        ))),
        None => Ok(()),
    }
}

/// The message positions `derived_from` names (`message:<seq>`, no chunk
/// suffix), or None when it names none.
pub fn derived_from(req: &UpdateContextRequest) -> Result<Option<Vec<u64>>, RecordError> {
    if req.derived_from.is_empty() {
        return Ok(None);
    }
    let mut seqs = Vec::with_capacity(req.derived_from.len());
    for id in &req.derived_from {
        match message::seq_of_content_id(id) {
            Some(seq) if !id.contains('#') => seqs.push(seq),
            _ => {
                return Err(invalid(format!(
                    "derived_from: '{id}' is not a message content id (message:<n>)"
                )));
            }
        }
    }
    Ok(Some(seqs))
}

/// The record a validated write becomes. `creates_entities` (names) and
/// `typed_entities` (name and type) are kept in sync: the entity graph
/// reads both.
pub fn record(req: UpdateContextRequest) -> ContextUpdate {
    let typed_entities: Vec<TypedEntity> = req
        .entities
        .iter()
        .map(|e| TypedEntity {
            name: e.name.clone(),
            entity_type: e.entity_type,
        })
        .collect();
    let creates_entities: Vec<String> = req.entities.into_iter().map(|e| e.name).collect();

    ContextUpdate {
        id: Uuid::new_v4(),
        timestamp: Utc::now(),
        update_type: req.interaction_type,
        content: req.content,
        related_code: req.code_reference,
        parent_update: None,
        user_marked_important: false,
        creates_entities,
        creates_relationships: req.relations,
        references_entities: Vec::new(),
        typed_entities,
        author: req.author,
        derived_from: req.derived_from,
        meta: req.meta,
    }
}

#[cfg(test)]
pub(crate) mod tests;
