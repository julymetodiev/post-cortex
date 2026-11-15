// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! Tool `update_conversation_context`: a write, an atomic bulk write
//! (`updates`), or a dry run of either (`dry_run`) through
//! `Records::validate`.

use std::collections::HashMap;

use crate::Memory;
use crate::capabilities::RecordError;
use post_cortex_core::core::context_update::{CodeReference, UpdateMeta};
use post_cortex_core::services::{BulkUpdateContextRequest, UpdateContextRequest};
use serde_json::Value;
use uuid::Uuid;

use crate::tools::args::{
    ContextUpdateItem, EntityItem, RelationItem, UpdateConversationContextRequest,
};
use crate::tools::check::{self, Problem};
use crate::tools::handlers::Outcome;
use crate::tools::parse::{FieldHint, arguments};
use crate::tools::reply;
use crate::tools::responses::{BulkItem, BulkWritten, RecordWritten};
use crate::tools::{Failure, Kind, wire};

mod content;

const HINTS: &[FieldHint] = &[
    FieldHint {
        field: "session_id",
        expected: "UUID string (36 chars with hyphens)",
        hint: "Create a session first using the 'session' tool with action='create', then use the returned UUID",
    },
    FieldHint {
        field: "interaction_type",
        expected: "one of: qa, decision_made, problem_solved, code_change, requirement_added, concept_defined",
        hint: "Valid interaction types: qa, decision_made, problem_solved, code_change, requirement_added, concept_defined",
    },
    FieldHint {
        field: "content",
        expected: "object with string key-value pairs",
        hint: "Content must be a map of string keys to string values. For complex data, stringify as JSON first.",
    },
];

/// One write of the call, in wire form.
struct Item<'a> {
    interaction_type: &'a str,
    content: &'a HashMap<String, String>,
    entities: &'a [EntityItem],
    relations: &'a [RelationItem],
    code_reference: Option<CodeReference>,
    derived_from: Vec<String>,
    meta: UpdateMeta,
}

pub async fn handle(memory: &Memory, params: Value, default_author: Option<String>) -> Outcome {
    let req: UpdateConversationContextRequest = arguments(params, HINTS)?;
    let session_id = check::session_id(&req.session_id)?;
    let author = crate::tools::handlers::author(req.author.as_deref(), default_author);
    let dry_run = req.dry_run.unwrap_or(false);
    match &req.updates {
        Some(updates) => bulk(memory, session_id, updates, author, dry_run).await,
        None => single(memory, session_id, &req, author, dry_run).await,
    }
}

async fn single(
    memory: &Memory,
    session_id: Uuid,
    req: &UpdateConversationContextRequest,
    author: Option<String>,
    dry_run: bool,
) -> Outcome {
    let interaction_type = req.interaction_type.as_deref().ok_or_else(|| {
        check::missing(
            "interaction_type",
            "one of: qa, decision_made, problem_solved, code_change, requirement_added, concept_defined",
            "For single update, provide 'interaction_type' and 'content'. For bulk updates, use 'updates' array instead.",
        )
    })?;
    check::interaction_type(interaction_type)?;
    let content = req.content.as_ref().ok_or_else(|| {
        check::missing(
            "content",
            "object with string key-value pairs",
            "For single update, provide 'content' and 'interaction_type'. For bulk updates, use 'updates' array instead.",
        )
    })?;
    let code_reference =
        crate::tools::code_ref::code_reference(req.code_reference.as_ref(), "code_reference")?;
    let meta = wire::update_meta(
        req.valid_from.as_deref(),
        req.supersedes.as_deref().unwrap_or_default(),
        req.confidence,
        "",
    )?;
    let has_code_reference = code_reference.is_some();
    let item = Item {
        interaction_type,
        content,
        entities: &req.entities,
        relations: &req.relations,
        code_reference,
        derived_from: req.derived_from.clone().unwrap_or_default(),
        meta,
    };
    let request = request(session_id, &item, author)?;
    if dry_run {
        // The same checks as the real write; nothing is stored.
        memory
            .records
            .validate(request)
            .await
            .map_err(|e| dry_run_failure(e, session_id))?;
        return reply::with(
            format!(
                "✅ Dry run - Request validation successful\n\
             Session ID: {}\n\
             Interaction Type: {}\n\
             Content fields: {}\n\
             Code Reference: {}\n\n\
             No changes were made. Set dry_run to false or omit it to actually save this update.",
                session_id,
                interaction_type,
                content.keys().cloned().collect::<Vec<_>>().join(", "),
                if has_code_reference {
                    "provided"
                } else {
                    "none"
                }
            ),
            &crate::tools::responses::DryRun {
                session_id: session_id.to_string(),
                records: 1,
                stored: false,
            },
        );
    }
    let written = memory.records.write(request).await.map_err(Failure::from)?;
    reply::with(
        "Context updated successfully",
        &RecordWritten {
            entry_id: written.entry_id.to_string(),
            session_id: written.session_id.to_string(),
            durable: written.durable,
        },
    )
}

async fn bulk(
    memory: &Memory,
    session_id: Uuid,
    updates: &[ContextUpdateItem],
    author: Option<String>,
    dry_run: bool,
) -> Outcome {
    // Malformed code references and validity statements are rejected up
    // front, in both modes.
    let mut items = Vec::with_capacity(updates.len());
    for (i, u) in updates.iter().enumerate() {
        items.push(Item {
            interaction_type: &u.interaction_type,
            content: &u.content,
            entities: &u.entities,
            relations: &u.relations,
            code_reference: crate::tools::code_ref::code_reference(
                u.code_reference.as_ref(),
                &format!("updates[{i}].code_reference"),
            )?,
            derived_from: u.derived_from.clone().unwrap_or_default(),
            meta: wire::update_meta(
                u.valid_from.as_deref(),
                u.supersedes.as_deref().unwrap_or_default(),
                u.confidence,
                &format!("updates[{i}]."),
            )?,
        });
    }
    // One outcome per input item, in input order; a malformed item fails
    // the whole batch (atomic, as the service's bulk write).
    let requests: Vec<Result<UpdateContextRequest, Failure>> = items
        .iter()
        .map(|item| request(session_id, item, author.clone()))
        .collect();
    if dry_run {
        let updates = requests
            .into_iter()
            .enumerate()
            .map(|(i, r)| {
                r.map_err(|f| {
                    let m = format!("updates[{i}]: {}", f.message);
                    f.with_message(m)
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        memory
            .records
            .validate_bulk(BulkUpdateContextRequest {
                session_id,
                updates,
            })
            .await
            .map_err(|e| dry_run_failure(e, session_id))?;
        let preview = preview(&items);
        return reply::with(
            format!(
                "✅ Dry run - Bulk update validation successful\n\
             Session ID: {}\n\
             Total updates: {}\n\n\
             Preview:\n{}\n\n\
             No changes were made. Set dry_run to false or omit it to actually save these updates.",
                session_id,
                items.len(),
                preview.join("\n")
            ),
            &crate::tools::responses::DryRun {
                session_id: session_id.to_string(),
                records: items.len(),
                stored: false,
            },
        );
    }
    let mut outcomes: Vec<Option<Result<Uuid, String>>> = vec![None; items.len()];
    let mut valid = Vec::with_capacity(items.len());
    for (index, r) in requests.into_iter().enumerate() {
        match r {
            Ok(req) => valid.push(req),
            Err(f) => outcomes[index] = Some(Err(f.message)),
        }
    }
    if valid.len() == items.len() {
        match memory
            .records
            .bulk_write(BulkUpdateContextRequest {
                session_id,
                updates: valid,
            })
            .await
        {
            Ok(resp) => {
                for (outcome, id) in outcomes.iter_mut().zip(resp.entry_ids) {
                    *outcome = Some(Ok(id));
                }
            }
            Err(e) => {
                let error = format!("Bulk persist failed: {e}");
                outcomes
                    .iter_mut()
                    .for_each(|o| *o = Some(Err(error.clone())));
            }
        }
    }
    bulk_reply(outcomes)
}

/// The reply of a bulk write: success when every item was written, a tool
/// error with the per-item results otherwise.
fn bulk_reply(outcomes: Vec<Option<Result<Uuid, String>>>) -> Outcome {
    let mut results = Vec::with_capacity(outcomes.len());
    let mut errors = Vec::new();
    for (index, outcome) in outcomes.into_iter().enumerate() {
        match outcome
            .unwrap_or_else(|| Err("not written: another item of the batch is invalid".into()))
        {
            Ok(id) => results.push(BulkItem {
                index,
                entry_id: Some(id.to_string()),
                error: None,
            }),
            Err(error) => {
                errors.push(format!("Update {index}: {error}"));
                results.push(BulkItem {
                    index,
                    entry_id: None,
                    error: Some(error),
                });
            }
        }
    }
    let error_count = errors.len();
    let success_count = results.len() - error_count;
    let response = BulkWritten {
        success_count,
        error_count,
        errors,
        results,
    };
    if error_count == 0 {
        return reply::with(
            format!("Bulk update completed successfully: {success_count} updates added"),
            &response,
        );
    }
    tracing::warn!(
        error_count,
        "bulk update_conversation_context: items failed"
    );
    reply::failed(
        format!(
            "Bulk update completed with errors: {success_count} succeeded, {error_count} failed"
        ),
        &response,
    )
}

/// The dry-run preview of a bulk write: each item's type and fields.
fn preview(items: &[Item<'_>]) -> Vec<String> {
    items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            format!(
                "  [{}] {} - fields: {}",
                i + 1,
                item.interaction_type,
                item.content.keys().cloned().collect::<Vec<_>>().join(", ")
            )
        })
        .collect()
}

/// The canonical write request of one item.
fn request(
    session_id: Uuid,
    item: &Item<'_>,
    author: Option<String>,
) -> Result<UpdateContextRequest, Failure> {
    let (interaction_type, content) = content::build(item.interaction_type, item.content)?;
    let now = chrono::Utc::now();
    Ok(UpdateContextRequest {
        session_id,
        interaction_type,
        content,
        // The tool documents that an unknown entity type is `concept`.
        entities: wire::entities(
            item.entities
                .iter()
                .map(|e| (e.name.as_str(), e.entity_type.as_str())),
            false,
            now,
        )?,
        relations: wire::relations(item.relations.iter().map(|r| {
            (
                r.from_entity.as_str(),
                r.to_entity.as_str(),
                r.relation_type.as_str(),
                r.context.as_str(),
            )
        }))?,
        code_reference: item.code_reference.clone(),
        author,
        derived_from: item.derived_from.clone(),
        meta: item.meta.clone(),
    })
}

/// The failure of a dry run that a real write would also refuse.
fn dry_run_failure(e: RecordError, session_id: Uuid) -> Failure {
    match e {
        RecordError::SessionNotFound(_) => {
            let received = Value::String(session_id.to_string());
            Failure::new(Kind::NotFound, "Session not found: session does not exist")
                .parameter("session_id")
                .hint("Create a session first using the 'session' tool with action='create'")
                .suggestions(check::suggestions(
                    "session_id",
                    Problem::NotFound,
                    Some(&received),
                ))
                .received(received)
        }
        RecordError::InvalidArgument(msg) => Failure::invalid(msg),
        e => Failure::from(e),
    }
}

#[cfg(test)]
mod entry_id_tests;
#[cfg(test)]
mod tests;
