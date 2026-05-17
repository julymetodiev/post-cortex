use crate::core::context_update::CodeReference;
use crate::core::memory_system::ConversationMemorySystem;
use crate::core::timeout_utils::with_mcp_timeout;
use crate::tools::mcp::{
    get_memory_system, interaction_to_context_update, string_to_anyhow, ContextUpdateItem,
    Interaction, MCPToolResult,
};
use anyhow::Result;
use std::collections::HashMap;
use tracing::{debug, error, info, instrument, warn};
use uuid::Uuid;

/// Build an [`Interaction`] from a flat `interaction_type + content` payload.
///
/// Centralises the slot-resolution and extras-collection logic that was once
/// duplicated verbatim between the single-update and bulk-update entry points
/// — both now delegate here.
///
/// `Ok(None)` is reserved for unknown interaction types so callers can shape
/// their own error message (single returns immediately, bulk records and
/// continues).
fn build_interaction(
    interaction_type: &str,
    content: &HashMap<String, String>,
) -> Result<Option<Interaction>> {
    let extract_extras = |exclude_keys: &[&str]| -> Vec<String> {
        content
            .iter()
            .filter(|(k, _)| !exclude_keys.contains(&k.as_str()))
            .map(|(k, v)| format!("{}: {}", k, v))
            .collect()
    };

    let resolve_slot = |preferred: &[&str], fallback_keys: &[&str]| -> String {
        for k in preferred.iter().chain(fallback_keys.iter()) {
            if let Some(v) = content.get(*k) {
                if !v.trim().is_empty() {
                    return v.clone();
                }
            }
        }
        String::new()
    };

    let interaction = match interaction_type {
        "qa" => {
            let question = resolve_slot(&["question"], &["title"]);
            let answer = resolve_slot(&["answer"], &["description"]);
            let extras = extract_extras(&["question", "answer", "title", "description"]);
            Interaction::QA {
                question,
                answer,
                details: extras,
            }
        }
        "code_change" => {
            let file_path = resolve_slot(&["file_path", "file"], &["title", "description"]);
            let changes = resolve_slot(
                &["changes", "diff", "change_type", "change"],
                &["description"],
            );
            let extras = extract_extras(&[
                "file_path",
                "file",
                "title",
                "description",
                "changes",
                "diff",
                "change_type",
                "change",
            ]);
            Interaction::CodeChange {
                file_path,
                diff: changes,
                details: extras,
            }
        }
        "problem_solved" => {
            let problem = resolve_slot(&["problem"], &["title"]);
            let solution = resolve_slot(&["solution"], &["description"]);
            let extras = extract_extras(&["problem", "solution", "title", "description"]);
            Interaction::ProblemSolved {
                problem,
                solution,
                details: extras,
            }
        }
        "decision_made" => {
            let decision = resolve_slot(&["decision"], &["title"]);
            let rationale = resolve_slot(&["rationale"], &["description"]);
            let extras = extract_extras(&["decision", "rationale", "title", "description"]);
            Interaction::DecisionMade {
                decision,
                rationale,
                details: extras,
            }
        }
        "requirement_added" => {
            let requirement = resolve_slot(&["requirement"], &["title"]);
            let priority = content
                .get("priority")
                .cloned()
                .unwrap_or_else(|| "medium".to_string());
            let extras = extract_extras(&["requirement", "priority", "title", "description"]);
            Interaction::RequirementAdded {
                requirement,
                priority,
                details: extras,
            }
        }
        "concept_defined" => {
            let concept = resolve_slot(&["concept"], &["title"]);
            let definition = resolve_slot(&["definition"], &["description"]);
            let extras = extract_extras(&["concept", "definition", "title", "description"]);
            Interaction::ConceptDefined {
                concept,
                definition,
                details: extras,
            }
        }
        _ => return Ok(None),
    };

    Ok(Some(interaction))
}

/// Parse the optional `entities` / `relationships` extras that the single
/// update path supports as freeform comma/space-separated strings.
fn parse_entity_extras(
    content: &HashMap<String, String>,
) -> (Vec<String>, Vec<crate::core::context_update::EntityRelationship>) {
    use crate::core::context_update::{EntityRelationship, RelationType};

    let entities: Vec<String> = content
        .get("entities")
        .map(|s| {
            s.split(|c| c == ',' || c == ' ')
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty() && t.len() > 2)
                .collect()
        })
        .unwrap_or_default();

    let relationships: Vec<EntityRelationship> = content
        .get("relationships")
        .map(|rels_str| {
            rels_str
                .split(',')
                .filter_map(|rel_part| {
                    let parts: Vec<&str> = rel_part.trim().split_whitespace().collect();
                    if parts.len() < 3 {
                        return None;
                    }
                    let from = parts[0].to_string();
                    let rel_type = match parts[1].to_uppercase().as_str() {
                        "DEPENDS_ON" => RelationType::DependsOn,
                        "IMPLEMENTS" => RelationType::Implements,
                        "CAUSES" | "CAUSED_BY" => RelationType::CausedBy,
                        "SOLVES" => RelationType::Solves,
                        "LEADS_TO" => RelationType::LeadsTo,
                        "REQUIRED_BY" => RelationType::RequiredBy,
                        "CONFLICTS_WITH" => RelationType::ConflictsWith,
                        _ => RelationType::RelatedTo,
                    };
                    Some(EntityRelationship {
                        from_entity: from,
                        to_entity: parts[2..].join(" "),
                        relation_type: rel_type,
                        context: String::new(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    (entities, relationships)
}

#[instrument(skip(system, content), fields(
    session_id = %session_id,
    interaction_type = %interaction_type,
    has_code_reference = code_reference.is_some()
))]
pub async fn update_conversation_context_with_system(
    interaction_type: String,
    content: HashMap<String, String>,
    code_reference: Option<CodeReference>,
    session_id: Uuid,
    system: &ConversationMemorySystem,
) -> Result<MCPToolResult> {
    info!("Starting update_conversation_context_with_system");
    debug!("Parsing interaction type: {}", interaction_type);

    let result = with_mcp_timeout(async {
        let interaction = match build_interaction(&interaction_type, &content)? {
            Some(i) => i,
            None => {
                error!("Unknown interaction type: {}", interaction_type);
                return Ok(MCPToolResult::error(format!(
                    "Unknown interaction type: {}",
                    interaction_type
                )));
            }
        };

        debug!("Converting interaction to ContextUpdate...");
        let mut update = interaction_to_context_update(interaction, code_reference)?;

        if update.content.title.trim().is_empty()
            && update.content.description.trim().is_empty()
        {
            let known_keys: Vec<String> = content.keys().cloned().collect();
            warn!(
                "Rejecting empty update for interaction_type='{}' (no usable title/description). Provided keys: {:?}",
                interaction_type, known_keys
            );
            return Ok(MCPToolResult::error(format!(
                "Refusing to store empty update for interaction_type='{}': none of the recognised keys carried text. \
                 Provide one of the type-specific keys (e.g. decision/rationale, problem/solution, concept/definition, \
                 question/answer, file/changes, requirement) or a generic 'title' and/or 'description'. \
                 Got keys: {:?}",
                interaction_type, known_keys
            )));
        }

        // Optional freeform `entities` / `relationships` overrides (single-update only).
        let (extra_entities, extra_relationships) = parse_entity_extras(&content);
        if !extra_entities.is_empty() {
            info!(
                "Parsed {} explicit entities: {:?}",
                extra_entities.len(),
                extra_entities
            );
            update.creates_entities = extra_entities;
        }
        if !extra_relationships.is_empty() {
            info!("Parsed {} explicit relationships", extra_relationships.len());
            // Relationship context defaults to the update title for traceability.
            let title = update.content.title.clone();
            update.creates_relationships = extra_relationships
                .into_iter()
                .map(|mut r| {
                    r.context = title.clone();
                    r
                })
                .collect();
        }

        info!(
            "Created ContextUpdate with {} entities and {} relationships",
            update.creates_entities.len(),
            update.creates_relationships.len()
        );

        debug!("Adding context update to session: {}", session_id);
        let text = format!("{}\n{}", update.content.title, update.content.description);
        let metadata = Some(
            serde_json::to_value(&update)
                .map_err(|e| anyhow::anyhow!("Failed to serialize update metadata: {}", e))?,
        );
        system
            .add_incremental_update(session_id, text, metadata)
            .await
            .map_err(string_to_anyhow)?;
        info!("system.add_context_update completed successfully!");

        Ok(MCPToolResult::success(
            "Context updated successfully".to_string(),
            None,
        ))
    })
    .await;

    match result {
        Ok(success_result) => success_result,
        Err(timeout_error) => {
            error!(
                "TIMEOUT: update_conversation_context_with_system - session: {}, error: {}",
                session_id, timeout_error
            );
            Ok(MCPToolResult::error(format!(
                "Operation timed out: {}",
                timeout_error
            )))
        }
    }
}

pub async fn bulk_update_conversation_context(
    updates: Vec<ContextUpdateItem>,
    session_id: Uuid,
) -> Result<MCPToolResult> {
    info!(
        "MCP-TOOLS: bulk_update_conversation_context() called with {} updates for session {}",
        updates.len(),
        session_id
    );

    let system = get_memory_system().await?;
    let mut success_count = 0;
    let mut error_count = 0;
    let mut errors: Vec<String> = Vec::new();

    for (index, update_item) in updates.into_iter().enumerate() {
        let interaction = match build_interaction(&update_item.interaction_type, &update_item.content) {
            Ok(Some(i)) => i,
            Ok(None) => {
                error_count += 1;
                errors.push(format!(
                    "Update {}: Unknown interaction type: {}",
                    index, update_item.interaction_type
                ));
                continue;
            }
            Err(e) => {
                error_count += 1;
                errors.push(format!("Update {}: Failed to build interaction: {}", index, e));
                continue;
            }
        };

        match interaction_to_context_update(interaction, update_item.code_reference) {
            Ok(update) => {
                if update.content.title.trim().is_empty()
                    && update.content.description.trim().is_empty()
                {
                    let known_keys: Vec<String> =
                        update_item.content.keys().cloned().collect();
                    error_count += 1;
                    errors.push(format!(
                        "Update {}: empty title and description for interaction_type='{}'. \
                         Provided keys: {:?}. Supply a recognised key or a generic 'title'/'description'.",
                        index, update_item.interaction_type, known_keys
                    ));
                    continue;
                }
                // Match the single-update path: feed vectorizer "title\ndescription"
                // (was just `description` before, which dropped half the signal
                // for bulk-written records and hurt cross-session search).
                let text = format!("{}\n{}", update.content.title, update.content.description);
                let metadata = match serde_json::to_value(&update) {
                    Ok(v) => Some(v),
                    Err(e) => {
                        error_count += 1;
                        errors.push(format!(
                            "Update {}: Failed to serialize metadata: {}",
                            index, e
                        ));
                        continue;
                    }
                };

                match system
                    .add_incremental_update(session_id, text, metadata)
                    .await
                {
                    Ok(_) => {
                        success_count += 1;
                        debug!("Update {} added successfully", index);
                    }
                    Err(e) => {
                        error_count += 1;
                        errors.push(format!("Update {}: Failed to add: {}", index, e));
                    }
                }
            }
            Err(e) => {
                error_count += 1;
                errors.push(format!("Update {}: Failed to convert: {}", index, e));
            }
        }
    }

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

    #[cfg(feature = "embeddings")]
    {
        if let Err(e) = system.auto_vectorize_if_enabled(session_id).await {
            tracing::warn!(
                "Auto-vectorization warning after bulk updates for session {}: {}",
                session_id, e
            );
        } else {
            tracing::info!(
                "Auto-vectorization completed after {} bulk updates for session {}",
                success_count, session_id
            );
        }
    }

    Ok(MCPToolResult::success(
        message,
        Some(serde_json::json!({
            "success_count": success_count,
            "error_count": error_count,
            "errors": errors
        })),
    ))
}

pub async fn update_conversation_context(
    interaction_type: String,
    content: HashMap<String, String>,
    code_reference: Option<CodeReference>,
    session_id: Uuid,
) -> Result<MCPToolResult> {
    info!("MCP-TOOLS: Getting memory system for update_conversation_context");
    let system = get_memory_system().await?;
    info!("MCP-TOOLS: Got memory system, delegating to update_conversation_context_with_system");

    update_conversation_context_with_system(
        interaction_type,
        content,
        code_reference,
        session_id,
        &system,
    )
    .await
}
