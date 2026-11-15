// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The retrieval port, [`SearchIndex`] (ARCHITECTURE section 5): the one
//! port the capabilities search and embed through. An index entry is a
//! session's content (an update, a chunk `<id>#<n>` of a long one, an
//! entity description, a captured turn) with its text, metadata and
//! vector; entries are keyed by (session, content id). How the adapter
//! indexes and persists them is its own choice.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::vectors::{AUTHOR_KEY, Corpus, VectorMetadata};

/// One stored entry: its vector and what it embeds. `meta.source` is the
/// session, `meta.id` the content id, `meta.content_type` the kind.
#[derive(Debug, Clone)]
pub struct IndexEntry {
    /// The embedding: any norm when upserted (the index compares by
    /// cosine); L2-normalised as the index returns it.
    pub vector: Vec<f32>,
    /// Session, content id, kind, text, time and metadata.
    pub meta: VectorMetadata,
}

/// A retrieved entry and its cosine similarity to the query vector.
#[derive(Debug, Clone)]
pub struct IndexHit {
    /// Cosine similarity to the query vector (0 for a lexical hit, whose
    /// order is the lexical one; see [`SearchIndex::lexical`]).
    pub similarity: f32,
    /// The entry, without its vector, shared with the index.
    pub meta: Arc<VectorMetadata>,
}

/// Which entries a retrieval covers: the sessions, then the entries of
/// them that pass [`Self::admits`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct IndexFilter {
    /// These sessions (order and duplicates do not matter); `None`: every
    /// session.
    pub sessions: Option<Vec<String>>,
    /// Records, messages or both.
    pub corpus: Corpus,
    /// Only entries dated within this range (inclusive).
    pub date_range: Option<post_cortex_core::time::TimeRange>,
    /// Only entries by this author ([`AUTHOR_KEY`] in their metadata).
    pub author: Option<String>,
}

impl IndexFilter {
    /// Whether `meta` passes: in the corpus, dated within the range, by
    /// the author (the session is the adapter's to check).
    pub fn admits(&self, meta: &VectorMetadata) -> bool {
        self.corpus.admits(&meta.content_type)
            && self
                .date_range
                .is_none_or(|(start, end)| meta.timestamp >= start && meta.timestamp <= end)
            && self
                .author
                .as_deref()
                .is_none_or(|a| meta.metadata.get(AUTHOR_KEY).is_some_and(|m| m == a))
    }

    /// Whether the entry's session is covered.
    pub fn covers(&self, session: &str) -> bool {
        self.sessions
            .as_ref()
            .is_none_or(|ids| ids.iter().any(|s| s == session))
    }
}

/// Which entries of a session [`SearchIndex::entries`] returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntrySelect {
    /// Every entry.
    All,
    /// These content ids, each with its chunks (`<id>#<n>`).
    ContentIds(Vec<String>),
    /// Entries of these kinds (`content_type`).
    Kinds(Vec<String>),
}

/// What loading the persisted entries at open stored and left out.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoadReport {
    /// Entries loaded.
    pub loaded: usize,
    /// Entries refused because their dimension is not the model's (stored
    /// by another embedding model).
    pub rejected_dimension: usize,
}

/// Counts of the index.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IndexStats {
    /// Entries in the index.
    pub entries: usize,
    /// Entries per session.
    pub per_session: HashMap<String, usize>,
    /// Dimension of the vectors the index holds.
    pub dimension: usize,
    /// Approximate memory the entries take, in bytes.
    pub memory_bytes: usize,
    /// What the load at open stored and refused.
    pub load: LoadReport,
}

/// Vector and lexical retrieval over the index entries, and their storage.
#[async_trait]
pub trait SearchIndex: Send + Sync {
    /// Store `entries` in one batch. Each replaces the entry stored under
    /// its (session, content id). An entry of another dimension than the
    /// index's fails the call and nothing is stored.
    async fn upsert(&self, entries: Vec<IndexEntry>) -> Result<()>;

    /// Delete the entries of `session` with these content ids, each with
    /// its chunks; other sessions' entries under the same ids stay.
    /// Returns how many entries were removed.
    async fn delete(&self, session: &str, content_ids: &[String]) -> Result<usize>;

    /// The stored entries of `session` that `select` names, in no
    /// particular order.
    async fn entries(&self, session: &str, select: &EntrySelect) -> Result<Vec<IndexEntry>>;

    /// The `k` entries most similar to `query` among those `filter`
    /// covers, best first.
    async fn nearest(&self, query: &[f32], filter: &IndexFilter, k: usize)
    -> Result<Vec<IndexHit>>;

    /// The similarity to `query` of the entries `content_ids` of
    /// `session`, in the order given; ids without an entry are left out.
    async fn score(
        &self,
        query: &[f32],
        session: &str,
        content_ids: &[String],
    ) -> Result<Vec<IndexHit>>;

    /// BM25 top-`k` for `query` among the entries `filter` covers, best
    /// first. The adapter picks the query's terms (words too common in the
    /// stored text to discriminate are dropped); no term left gives no hit.
    /// Hits carry similarity 0: rank by their order.
    async fn lexical(&self, query: &str, filter: &IndexFilter, k: usize) -> Result<Vec<IndexHit>>;

    /// Counts and the load report. Opens the index if it is not open yet;
    /// fails when the open failed or found only another model's vectors.
    async fn stats(&self) -> Result<IndexStats>;
}
