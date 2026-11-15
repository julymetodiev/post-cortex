// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Record responses: writes, bulk writes, delete / forget / purge.

use post_cortex_core::core::context_update::CodeReference;
use serde::Serialize;
use uuid::Uuid;

use crate::tools::responses::search::Ended;

/// A record written.
#[derive(Debug, Clone, Serialize)]
pub struct RecordWritten {
    /// The record's id (`entry_id`; usable in `supersedes`, forget, purge).
    pub entry_id: String,
    /// The session.
    pub session_id: String,
    /// It is stored.
    pub durable: bool,
}

/// A bulk write: one result per input item, in input order. The batch is
/// atomic: a bad item fails every item and nothing is written.
#[derive(Debug, Clone, Serialize)]
pub struct BulkWritten {
    /// Items written.
    pub success_count: usize,
    /// Items not written.
    pub error_count: usize,
    /// `Update <index>: <error>` per failed item.
    pub errors: Vec<String>,
    /// One per input item.
    pub results: Vec<BulkItem>,
}

/// The outcome of one item of a bulk write.
#[derive(Debug, Clone, Serialize)]
pub struct BulkItem {
    /// Its position in the input.
    pub index: usize,
    /// Its record's id, when written.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entry_id: Option<String>,
    /// Why it was not written.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// A record deleted, forgotten or purged.
#[derive(Debug, Clone, Serialize)]
pub struct RecordChanged {
    /// The record was there (and, for forget, still valid).
    pub existed: bool,
    /// The record.
    pub entry_id: String,
    /// The session.
    pub session_id: String,
}

/// A write that was validated and not stored (`dry_run`).
#[derive(Debug, Clone, Serialize)]
pub struct DryRun {
    /// The session the write names.
    pub session_id: String,
    /// How many records it holds.
    pub records: usize,
    /// Always false: nothing was stored.
    pub stored: bool,
}

/// One record whole, as `query_conversation_context` `record` reads it:
/// what a search's snippet left out.
#[derive(Debug, Clone, Serialize)]
pub struct RecordWhole {
    /// The record's id.
    pub entry_id: String,
    /// Its session.
    pub session_id: String,
    /// Decision, problem solved, code change, ...
    pub update_type: post_cortex_core::core::context_update::UpdateType,
    /// Its title.
    pub title: String,
    /// Its description, whole.
    pub description: String,
    /// Its details.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub details: Vec<String>,
    /// Its examples.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub examples: Vec<String>,
    /// Its implications.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub implications: Vec<String>,
    /// The entities it names.
    pub entities: Vec<String>,
    /// The relations it asserts.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub relations: Vec<RecordRelationWhole>,
    /// Who wrote it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// When it was written.
    pub created_at: String,
    /// The writer's confidence.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
    /// The code it points at.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code_reference: Option<CodeReference>,
    /// The captured messages it was drawn from (`message:<seq>`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub derived_from: Vec<String>,
    /// The records the writer said it replaces.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub supersedes: Vec<Uuid>,
    /// The records whose validity it ended, oldest first.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ended_records: Vec<Uuid>,
    /// When and why it stopped holding, for a record that no longer does.
    #[serde(flatten)]
    pub ended: Option<Ended>,
    /// Marked important by the user.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub important: bool,
}

/// A relation as a record asserts it.
#[derive(Debug, Clone, Serialize)]
pub struct RecordRelationWhole {
    /// Source entity.
    pub from_entity: String,
    /// Target entity.
    pub to_entity: String,
    /// `depends_on`, `solves`, ...
    pub relation_type: &'static str,
    /// Why it holds, as the record says.
    pub context: String,
}

impl From<crate::graph_views::detail::RecordDetail> for RecordWhole {
    fn from(r: crate::graph_views::detail::RecordDetail) -> Self {
        Self {
            entry_id: r.id.to_string(),
            session_id: r.session_id.to_string(),
            update_type: r.update_type,
            title: r.title,
            description: r.description,
            details: r.details,
            examples: r.examples,
            implications: r.implications,
            entities: r.entities.into_iter().map(|e| e.name).collect(),
            relations: r
                .relations
                .into_iter()
                .map(|rel| RecordRelationWhole {
                    from_entity: rel.relation.from,
                    to_entity: rel.relation.to,
                    relation_type: rel.relation.relation_type.as_str(),
                    context: rel.context,
                })
                .collect(),
            author: r.author,
            created_at: crate::tools::responses::rfc3339(r.created_at),
            confidence: r.confidence,
            code_reference: r.code,
            derived_from: r.derived_from,
            supersedes: r.supersedes,
            ended_records: r.ended,
            ended: (!r.validity.is_current()).then(|| Ended::of(&r.validity)),
            important: r.important,
        }
    }
}
