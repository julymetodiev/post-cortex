// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Pages of newest-first lists with an opaque cursor: the record browser
//! (filtered pages of a scope's records) and the files the records name.

use std::collections::HashSet;
use std::fmt;
use std::str::FromStr;

use chrono::DateTime;
use post_cortex_core::graph::temporal::Validity;
use uuid::Uuid;

use crate::graph_views::Index;
use crate::graph_views::detail::types::{
    FileTouch, Files, PageCursor, RecordFilter, RecordPage, ValidityFilter,
};

/// A cursor that [`PageCursor`]'s `Display` did not make.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("cursor: malformed")]
pub struct InvalidCursor;

/// `<seconds>.<nanoseconds>_<key>`: exact, so a page never repeats or skips
/// an item of the same second. The time has no `_`, so the key may.
impl<K: fmt::Display> fmt::Display for PageCursor<K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}.{:09}_{}",
            self.at.timestamp(),
            self.at.timestamp_subsec_nanos(),
            self.key
        )
    }
}

impl<K: FromStr> FromStr for PageCursor<K> {
    type Err = InvalidCursor;

    fn from_str(raw: &str) -> Result<Self, InvalidCursor> {
        let (time, key) = raw.split_once('_').ok_or(InvalidCursor)?;
        let (secs, nanos) = time.split_once('.').ok_or(InvalidCursor)?;
        let secs: i64 = secs.parse().map_err(|_| InvalidCursor)?;
        let nanos: u32 = nanos.parse().map_err(|_| InvalidCursor)?;
        Ok(Self {
            at: DateTime::from_timestamp(secs, nanos).ok_or(InvalidCursor)?,
            key: key.parse().map_err(|_| InvalidCursor)?,
        })
    }
}

/// The items of `sorted` (newest first by `cursor_of`, descending) after
/// `after`, at most `limit`, and where the next page starts (`None` on the
/// last).
pub fn page_after<T, K: Ord>(
    sorted: impl IntoIterator<Item = T>,
    cursor_of: impl Fn(&T) -> PageCursor<K>,
    after: Option<&PageCursor<K>>,
    limit: usize,
) -> (Vec<T>, Option<PageCursor<K>>) {
    let mut page: Vec<T> = sorted
        .into_iter()
        .filter(|item| after.is_none_or(|after| cursor_of(item) < *after))
        .take(limit + 1)
        .collect();
    if page.len() <= limit {
        return (page, None);
    }
    page.truncate(limit);
    let next = page.last().map(&cursor_of);
    (page, next)
}

impl Index<'_> {
    /// A page of the records matching `filter`, newest first, after
    /// `cursor`, at most `limit`, with the total over all pages.
    pub fn record_page(
        &self,
        filter: &RecordFilter,
        limit: usize,
        cursor: Option<PageCursor<Uuid>>,
    ) -> RecordPage {
        let mentioning: Option<HashSet<Uuid>> = filter.entity.as_deref().map(|name| {
            self.entity(name)
                .and_then(|e| self.graph.entity_mentions.get(&e.name))
                .into_iter()
                .flatten()
                .copied()
                .collect()
        });
        let text = filter.text.trim().to_lowercase();
        let matches = |id: &Uuid| -> bool {
            let Some((session_id, u)) = self.records.get(id) else {
                return false;
            };
            let current = self.validity.get(id).is_none_or(Validity::is_current);
            (filter.types.is_empty() || filter.types.contains(&u.update_type))
                && filter.session.is_none_or(|s| s == *session_id)
                && filter
                    .author
                    .as_deref()
                    .is_none_or(|a| u.author.as_deref() == Some(a))
                && mentioning.as_ref().is_none_or(|ids| ids.contains(id))
                && filter.from.is_none_or(|f| u.timestamp >= f)
                && filter.to.is_none_or(|t| u.timestamp <= t)
                && match filter.validity {
                    ValidityFilter::Any => true,
                    ValidityFilter::Current => current,
                    ValidityFilter::Ended => !current,
                }
                && (text.is_empty()
                    || std::iter::once(&u.content.title)
                        .chain(std::iter::once(&u.content.description))
                        .chain(&u.content.details)
                        .any(|s| s.to_lowercase().contains(&text)))
        };
        let matching: Vec<PageCursor<Uuid>> = self
            .newest_first()
            .into_iter()
            .filter(|c| matches(&c.key))
            .collect();
        let total = matching.len();
        let (page, next) = page_after(matching, |c| *c, cursor.as_ref(), limit);
        RecordPage {
            records: self.record_details(page.into_iter().map(|c| c.key)),
            total,
            next,
        }
    }

    /// A page of the files the scope's records name (their code references
    /// and the files of their code changes) whose path contains `text` in
    /// any case, most recently touched first, after `cursor`, at most
    /// `limit`, with the total over all pages.
    pub fn files(&self, text: &str, limit: usize, cursor: Option<PageCursor<String>>) -> Files {
        let text = text.trim().to_lowercase();
        let mut files = self.file_touches(self.records.keys());
        files.retain(|f| f.path.to_lowercase().contains(&text));
        let total = files.len();
        let at = |f: &FileTouch| PageCursor {
            at: f.last_at,
            key: f.path.clone(),
        };
        let (files, next) = page_after(files, at, cursor.as_ref(), limit);
        Files { files, total, next }
    }
}
