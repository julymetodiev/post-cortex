// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Views derived from the records on read. None is stored, so none can
//! disagree with the records.

use crate::collections::SharedVecIter;
use crate::core::context_update::{CodeReference, ContextUpdate};
use crate::core::structured_context::StructuredContext;
use crate::session::active_session::ActiveSession;

use chrono::{DateTime, Utc};
use std::collections::BTreeMap;
use std::sync::Arc;
use uuid::Uuid;

/// A change event of the session: one per record.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ChangeRecord {
    /// Identifier of the change (the record's id for derived changes)
    pub id: Uuid,
    /// When the change occurred
    pub timestamp: DateTime<Utc>,
    /// The record's update type, by its stored name ("DecisionMade")
    pub change_type: String,
    /// The record's description
    pub description: String,
    /// The record the change is
    pub related_update_id: Option<Uuid>,
}

impl ActiveSession {
    /// The newest records, oldest first: at most the preferences'
    /// `max_hot_context_size`.
    pub fn hot_records(&self) -> SharedVecIter<'_, Arc<ContextUpdate>> {
        let max = self.user_preferences().max_hot_context_size;
        self.records
            .iter_from(self.records.len().saturating_sub(max))
    }

    /// The structured view of the records (decisions, questions, concepts,
    /// specifications, flow); see [`StructuredContext::from_records`].
    pub fn current_state(&self) -> StructuredContext {
        StructuredContext::from_records(self.records.iter().map(|u| &**u))
    }

    /// The records' code references by file path, in record order.
    pub fn code_references(&self) -> BTreeMap<String, Vec<CodeReference>> {
        let mut by_file: BTreeMap<String, Vec<CodeReference>> = BTreeMap::new();
        for code in self.records.iter().filter_map(|u| u.related_code.as_ref()) {
            by_file
                .entry(code.file_path.clone())
                .or_default()
                .push(code.clone());
        }
        by_file
    }

    /// One change per record, oldest first.
    pub fn change_history(&self) -> Vec<ChangeRecord> {
        self.records
            .iter()
            .map(|u| ChangeRecord {
                id: u.id,
                timestamp: u.timestamp,
                change_type: u.update_type.stored_name().to_string(),
                description: u.content.description.clone(),
                related_update_id: Some(u.id),
            })
            .collect()
    }

    /// The ten newest records' descriptions as a bullet list, newest first.
    pub fn context_summary(&self) -> String {
        let lines: Vec<String> = self
            .hot_records()
            .rev()
            .take(10)
            .map(|u| format!("- {}", u.content.description))
            .collect();
        if lines.is_empty() {
            "No context available".to_string()
        } else {
            lines.join("\n")
        }
    }
}
