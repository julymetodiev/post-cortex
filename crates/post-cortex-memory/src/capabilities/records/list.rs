// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Listing a session's records: filtered by type and time, with the total
//! the filter matches and each record's registered source reference.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use post_cortex_core::core::context_update::{ContextUpdate, UpdateType};
use post_cortex_core::core::freshness::SourceReference;
use uuid::Uuid;

use crate::capabilities::records::{RecordError, Records};

/// Records listed when the request sets no limit.
pub(crate) const DEFAULT_LIST_LIMIT: usize = 50;

/// Which records of a session to list.
#[derive(Debug, Clone, Default)]
pub struct ListRecords {
    /// Only records of these types; empty means every type.
    pub types: Vec<UpdateType>,
    /// Only records written strictly after this time.
    pub after: Option<DateTime<Utc>>,
    /// At most this many ([`DEFAULT_LIST_LIMIT`] when 0).
    pub limit: usize,
}

/// One page of a session's records, oldest first.
#[derive(Debug, Clone)]
pub struct RecordPage {
    /// The records, each with its source reference when one is registered.
    pub records: Vec<(Arc<ContextUpdate>, Option<SourceReference>)>,
    /// How many records the filter matches (the page holds at most
    /// `limit` of them).
    pub total: usize,
}

impl Records {
    /// The records of a session that `req` selects, oldest first.
    ///
    /// # Errors
    /// `SessionNotFound`, or the source references could not be read.
    pub async fn list(
        &self,
        session_id: Uuid,
        req: ListRecords,
    ) -> Result<RecordPage, RecordError> {
        let session = self.handle(session_id).await?.load_full();
        let limit = match req.limit {
            0 => DEFAULT_LIST_LIMIT,
            n => n,
        };
        let selected: Vec<&Arc<ContextUpdate>> = session
            .records()
            .iter()
            .filter(|r| req.types.is_empty() || req.types.contains(&r.update_type))
            .filter(|r| req.after.is_none_or(|after| r.timestamp > after))
            .collect();
        let total = selected.len();
        let page: Vec<Arc<ContextUpdate>> = selected.into_iter().take(limit).cloned().collect();
        let sources = self.sources_of(&page).await?;
        let records = page
            .into_iter()
            .map(|r| {
                let source = sources.get(&r.id.to_string()).cloned();
                (r, source)
            })
            .collect();
        Ok(RecordPage { records, total })
    }

    /// The registered source references of `records`, by entry id: one
    /// read per file the records cite.
    async fn sources_of(
        &self,
        records: &[Arc<ContextUpdate>],
    ) -> Result<HashMap<String, SourceReference>, RecordError> {
        let mut files: Vec<&str> = records
            .iter()
            .filter_map(|r| r.related_code.as_ref().map(|c| c.file_path.as_str()))
            .collect();
        files.sort_unstable();
        files.dedup();
        let mut out = HashMap::new();
        for file in files {
            let refs = self
                .rt
                .stores
                .freshness
                .get_entries_by_source(file)
                .await
                .map_err(crate::capabilities::records::storage)?;
            out.extend(refs.into_iter().map(|r| (r.entry_id.clone(), r)));
        }
        out.retain(|id, _| records.iter().any(|r| r.id.to_string() == *id));
        Ok(out)
    }
}
