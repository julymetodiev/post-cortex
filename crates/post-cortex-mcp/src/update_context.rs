//! MCP-side adapter for context-update writes.
//!
//! Phase 6 of the single-entrypoint migration: every MCP-driven write
//! flows through [`post_cortex_memory::services::MemoryServiceImpl`] —
//! the canonical [`PostCortexService`](post_cortex_core::services::PostCortexService)
//! implementation. This module only
//! translates the LLM-friendly wire format (HashMap content + typed
//! `entities` / `relations` arrays) into the canonical
//! [`UpdateContextRequest`](post_cortex_core::services::UpdateContextRequest);
//! validation, persistence, and metadata
//! shaping all happen inside the service.

use anyhow::{Result, anyhow};
use post_cortex_core::core::context_update::{
    CodeReference, EntityData, EntityRelationship, EntityType, RelationType, UpdateContent,
    UpdateType,
};
use post_cortex_core::core::timeout_utils::with_mcp_timeout;
use post_cortex_core::services::{
    BulkUpdateContextRequest as ServiceBulkRequest, PostCortexService,
    UpdateContextRequest as ServiceUpdateRequest,
};
use std::collections::HashMap;
use tracing::{debug, error, info, instrument, warn};
use uuid::Uuid;

use crate::{ContextUpdateItem, EntityItem, MCPToolResult, RelationItem, get_service};

/// Parse the LLM-provided `interaction_type + content` HashMap into a
/// typed [`UpdateContent`] using the same key-resolution conventions the
/// old MCP path used. Returns `Err` if the `interaction_type` is unknown
/// so callers can surface a precise error.
fn build_content(
    interaction_type: &str,
    content: &HashMap<String, String>,
) -> Result<(UpdateType, UpdateContent)> {
    let extract_extras = |exclude_keys: &[&str]| -> Vec<String> {
        content
            .iter()
            .filter(|(k, _)| !exclude_keys.contains(&k.as_str()))
            .map(|(k, v)| format!("{}: {}", k, v))
            .collect()
    };

    let resolve_slot = |preferred: &[&str], fallback_keys: &[&str]| -> String {
        for k in preferred.iter().chain(fallback_keys.iter()) {
            if let Some(v) = content.get(*k)
                && !v.trim().is_empty()
            {
                return v.clone();
            }
        }
        String::new()
    };

    let (update_type, title, description, details, implications) = match interaction_type {
        "qa" => {
            let title = resolve_slot(&["question"], &["title"]);
            let description = resolve_slot(&["answer"], &["description"]);
            let details = extract_extras(&["question", "answer", "title", "description"]);
            (
                UpdateType::QuestionAnswered,
                title,
                description,
                details,
                vec![],
            )
        }
        "code_change" => {
            let title = resolve_slot(&["file_path", "file"], &["title", "description"]);
            let description = resolve_slot(
                &["changes", "diff", "change_type", "change"],
                &["description"],
            );
            let details = extract_extras(&[
                "file_path",
                "file",
                "title",
                "description",
                "changes",
                "diff",
                "change_type",
                "change",
            ]);
            (
                UpdateType::CodeChanged,
                title,
                description,
                details,
                vec!["Code functionality updated".to_string()],
            )
        }
        "problem_solved" => {
            let title = resolve_slot(&["problem"], &["title"]);
            let description = resolve_slot(&["solution"], &["description"]);
            let details = extract_extras(&["problem", "solution", "title", "description"]);
            (
                UpdateType::ProblemSolved,
                title,
                description,
                details,
                vec!["Problem resolved".to_string()],
            )
        }
        "decision_made" => {
            let title = resolve_slot(&["decision"], &["title"]);
            let description = resolve_slot(&["rationale"], &["description"]);
            let details = extract_extras(&["decision", "rationale", "title", "description"]);
            (
                UpdateType::DecisionMade,
                title,
                description,
                details,
                vec![],
            )
        }
        "requirement_added" => {
            let title = resolve_slot(&["requirement"], &["title"]);
            let description = resolve_slot(&["description"], &[]);
            let details = extract_extras(&["requirement", "priority", "title", "description"]);
            (
                UpdateType::RequirementAdded,
                title,
                description,
                details,
                vec![],
            )
        }
        "concept_defined" => {
            let title = resolve_slot(&["concept"], &["title"]);
            let description = resolve_slot(&["definition"], &["description"]);
            let details = extract_extras(&["concept", "definition", "title", "description"]);
            (
                UpdateType::ConceptDefined,
                title,
                description,
                details,
                vec![],
            )
        }
        other => return Err(anyhow!("Unknown interaction type: {}", other)),
    };

    Ok((
        update_type,
        UpdateContent {
            title,
            description,
            details,
            examples: vec![],
            implications,
        },
    ))
}

/// Parse an MCP `entity_type` string (lowercase) into [`EntityType`].
/// Unknown values default to `Concept` to match the gRPC parser.
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

/// Parse an MCP `relation_type` string (lowercase) into [`RelationType`].
/// Returns `None` for unknown values — the caller surfaces this as an
/// `InvalidArgument` error rather than silently defaulting.
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

fn entities_to_domain(items: &[EntityItem]) -> Vec<EntityData> {
    let now = chrono::Utc::now();
    items
        .iter()
        .map(|e| EntityData {
            name: e.name.clone(),
            entity_type: parse_entity_type(&e.entity_type),
            first_mentioned: now,
            last_mentioned: now,
            mention_count: 1,
            importance_score: 1.0,
            description: None,
        })
        .collect()
}

fn relations_to_domain(items: &[RelationItem]) -> Result<Vec<EntityRelationship>> {
    let mut out = Vec::with_capacity(items.len());
    for (i, r) in items.iter().enumerate() {
        let rt = parse_relation_type(&r.relation_type).ok_or_else(|| {
            anyhow!(
                "relation[{i}]: unknown relation_type {:?}; valid values are: \
                 required_by, leads_to, related_to, conflicts_with, depends_on, implements, caused_by, solves",
                r.relation_type
            )
        })?;
        out.push(EntityRelationship {
            from_entity: r.from_entity.clone(),
            to_entity: r.to_entity.clone(),
            relation_type: rt,
            context: r.context.clone(),
        });
    }
    Ok(out)
}

/// Build a canonical [`ServiceUpdateRequest`] from the MCP wire payload.
fn build_request(
    session_id: Uuid,
    interaction_type: &str,
    content: &HashMap<String, String>,
    entities: &[EntityItem],
    relations: &[RelationItem],
    code_reference: Option<CodeReference>,
) -> Result<ServiceUpdateRequest> {
    let (update_type, update_content) = build_content(interaction_type, content)?;
    Ok(ServiceUpdateRequest {
        session_id,
        interaction_type: update_type,
        content: update_content,
        entities: entities_to_domain(entities),
        relations: relations_to_domain(relations)?,
        code_reference,
    })
}

/// Record a single context update via the canonical
/// [`PostCortexService::update_context`] path.
#[instrument(skip(content, entities, relations), fields(
    session_id = %session_id,
    interaction_type = %interaction_type,
    entities_count = entities.len(),
    relations_count = relations.len(),
    has_code_reference = code_reference.is_some()
))]
pub async fn update_conversation_context(
    interaction_type: String,
    content: HashMap<String, String>,
    entities: Vec<EntityItem>,
    relations: Vec<RelationItem>,
    code_reference: Option<CodeReference>,
    session_id: Uuid,
) -> Result<MCPToolResult> {
    info!("MCP-TOOLS: update_conversation_context() called");
    let service = get_service().await?;

    let req = match build_request(
        session_id,
        &interaction_type,
        &content,
        &entities,
        &relations,
        code_reference,
    ) {
        Ok(r) => r,
        Err(e) => {
            error!("update_conversation_context: bad input — {}", e);
            return Ok(MCPToolResult::error(e.to_string()));
        }
    };

    let result = with_mcp_timeout(async {
        match service.update_context(req).await {
            Ok(resp) => {
                debug!(
                    "update_conversation_context: persisted entry {} in session {}",
                    resp.entry_id, resp.session_id
                );
                Ok(MCPToolResult::success(
                    "Context updated successfully".to_string(),
                    None,
                ))
            }
            Err(e) => {
                warn!("update_conversation_context: service rejected — {}", e);
                Ok(MCPToolResult::error(e.to_string()))
            }
        }
    })
    .await;

    match result {
        Ok(r) => r,
        Err(timeout_error) => {
            error!(
                "TIMEOUT: update_conversation_context — session: {}, error: {}",
                session_id, timeout_error
            );
            Ok(MCPToolResult::error(format!(
                "Operation timed out: {}",
                timeout_error
            )))
        }
    }
}

/// Record multiple context updates in a single batch via the canonical
/// service. Items that fail translation or persistence are reported in
/// the response payload — the rest still land, matching the legacy
/// gRPC bulk semantics.
pub async fn bulk_update_conversation_context(
    updates: Vec<ContextUpdateItem>,
    session_id: Uuid,
) -> Result<MCPToolResult> {
    info!(
        "MCP-TOOLS: bulk_update_conversation_context() called with {} updates for session {}",
        updates.len(),
        session_id
    );

    let service = get_service().await?;

    let mut requests = Vec::with_capacity(updates.len());
    let mut error_count = 0;
    let mut errors: Vec<String> = Vec::new();
    for (index, item) in updates.iter().enumerate() {
        match build_request(
            session_id,
            &item.interaction_type,
            &item.content,
            &item.entities,
            &item.relations,
            item.code_reference.clone(),
        ) {
            Ok(req) => requests.push(req),
            Err(e) => {
                error_count += 1;
                errors.push(format!("Update {}: {}", index, e));
            }
        }
    }

    // Persist via the canonical bulk method when every translation
    // succeeded; otherwise fall back to per-item calls so partial
    // failure semantics are preserved.
    let success_count = if errors.is_empty() {
        match service
            .bulk_update_context(ServiceBulkRequest {
                session_id,
                updates: requests,
            })
            .await
        {
            Ok(resp) => resp.entry_ids.len(),
            Err(e) => {
                errors.push(format!("Bulk persist failed: {}", e));
                error_count += 1;
                0
            }
        }
    } else {
        // At least one item failed translation: keep the legacy
        // "best effort" behaviour and persist the good ones one at a
        // time so the caller still gets partial progress.
        let mut count = 0;
        for (offset, req) in requests.into_iter().enumerate() {
            match service.update_context(req).await {
                Ok(_) => count += 1,
                Err(e) => {
                    error_count += 1;
                    errors.push(format!("Update (translated index {offset}): {}", e));
                }
            }
        }
        count
    };

    let message = if error_count == 0 {
        format!(
            "Bulk update completed successfully: {} updates added",
            success_count
        )
    } else {
        format!(
            "Bulk update completed with errors: {} succeeded, {} failed",
            success_count, error_count
        )
    };

    Ok(MCPToolResult::success(
        message,
        Some(serde_json::json!({
            "success_count": success_count,
            "error_count": error_count,
            "errors": errors,
        })),
    ))
}
