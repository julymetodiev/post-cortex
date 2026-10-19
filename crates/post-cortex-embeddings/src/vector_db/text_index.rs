// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Lexical statistics and an optional BM25 index over the vectors' text.
//!
//! Document frequencies are always kept (they are a dictionary, not
//! postings): they decide which query words are too common to search for,
//! for every backend, in place of stopword lists. On LoCoMo dropping words on
//! more than 20% of the records matches a hand-made English stopword list
//! (fast-mode all@50 0.809 vs 0.812) at a third of its query cost, and works
//! in any language.
//!
//! Postings are kept only with `VectorDbConfig::full_text_index`, for storage
//! backends without full-text search (RocksDB; SurrealDB has a native BM25
//! index). Scoring as SurrealDB's: k1 1.2, b 0.75, statistics over the whole
//! index. Unlike SurrealDB's analyzer it does not fold accents.

use std::collections::HashMap;

use parking_lot::RwLock;

use crate::text::tokens;

const K1: f32 = 1.2;
const B: f32 = 0.75;
/// Query words on more than this share of the records are not searched for.
const MAX_QUERY_DF_SHARE: f32 = 0.2;
/// ...unless they are on at most this many records, so a small memory keeps
/// every word.
const MIN_PRUNED_DF: u32 = 10;

#[derive(Default)]
struct Inner {
    /// Term -> number of records containing it.
    df: HashMap<String, u32>,
    /// Records indexed.
    docs: u32,
    /// Term -> (vector id, term frequency in that vector's text); empty
    /// unless postings are kept.
    postings: HashMap<String, Vec<(u32, u32)>>,
    /// Vector id -> number of words in its text (with postings only).
    lengths: HashMap<u32, u32>,
    total_length: u64,
}

/// Document frequencies, and BM25 postings when enabled, keyed by vector id.
pub(super) struct TextIndex {
    postings: bool,
    inner: RwLock<Inner>,
}

fn term_counts(text: &str) -> HashMap<String, u32> {
    let mut counts = HashMap::new();
    for term in tokens(text) {
        *counts.entry(term).or_default() += 1;
    }
    counts
}

impl TextIndex {
    /// Index that keeps BM25 postings only when `postings` is set.
    pub(super) fn new(postings: bool) -> Self {
        Self {
            postings,
            inner: RwLock::default(),
        }
    }

    /// Whether BM25 search is available.
    pub(super) fn has_postings(&self) -> bool {
        self.postings
    }

    pub(super) fn add(&self, id: u32, text: &str) {
        let counts = term_counts(text);
        let mut inner = self.inner.write();
        inner.docs += 1;
        for term in counts.keys() {
            *inner.df.entry(term.clone()).or_default() += 1;
        }
        if !self.postings {
            return;
        }
        let length: u32 = counts.values().sum();
        for (term, tf) in counts {
            inner.postings.entry(term).or_default().push((id, tf));
        }
        if let Some(old) = inner.lengths.insert(id, length) {
            inner.total_length -= u64::from(old);
        }
        inner.total_length += u64::from(length);
    }

    /// Remove `id`, indexed from `text` (the same text it was added with).
    pub(super) fn remove(&self, id: u32, text: &str) {
        let counts = term_counts(text);
        let mut inner = self.inner.write();
        inner.docs = inner.docs.saturating_sub(1);
        for term in counts.keys() {
            if let Some(n) = inner.df.get_mut(term) {
                *n -= 1;
                if *n == 0 {
                    inner.df.remove(term);
                }
            }
            if let Some(list) = inner.postings.get_mut(term) {
                list.retain(|&(v, _)| v != id);
                if list.is_empty() {
                    inner.postings.remove(term);
                }
            }
        }
        if let Some(old) = inner.lengths.remove(&id) {
            inner.total_length -= u64::from(old);
        }
    }

    pub(super) fn clear(&self) {
        *self.inner.write() = Inner::default();
    }

    /// The words of `query` worth searching for, deduplicated: those on more
    /// than [`MAX_QUERY_DF_SHARE`] of the records (and more than
    /// [`MIN_PRUNED_DF`]) are dropped.
    pub(super) fn query_terms(&self, query: &str) -> Vec<String> {
        let mut terms = tokens(query);
        terms.sort_unstable();
        terms.dedup();
        let inner = self.inner.read();
        let max_df = ((MAX_QUERY_DF_SHARE * inner.docs as f32) as u32).max(MIN_PRUNED_DF);
        terms.retain(|t| inner.df.get(t).copied().unwrap_or(0) <= max_df);
        terms
    }

    /// The `k` best vectors for `terms` by BM25, best first, among those
    /// `keep` accepts. Empty without postings.
    pub(super) fn search(
        &self,
        terms: &[String],
        k: usize,
        keep: impl Fn(u32) -> bool,
    ) -> Vec<(u32, f32)> {
        let inner = self.inner.read();
        let docs = inner.lengths.len() as f32;
        if terms.is_empty() || k == 0 || docs == 0.0 {
            return Vec::new();
        }
        let avg_length = inner.total_length as f32 / docs;

        let mut scores: HashMap<u32, f32> = HashMap::new();
        for term in terms {
            let Some(list) = inner.postings.get(term) else {
                continue;
            };
            let df = list.len() as f32;
            let idf = ((docs - df + 0.5) / (df + 0.5) + 1.0).ln();
            for &(id, tf) in list {
                if !keep(id) {
                    continue;
                }
                let length = inner.lengths.get(&id).copied().unwrap_or(0) as f32;
                let tf = tf as f32;
                let norm = tf * (K1 + 1.0) / (tf + K1 * (1.0 - B + B * length / avg_length));
                *scores.entry(id).or_default() += idf * norm;
            }
        }
        let mut ranked: Vec<(u32, f32)> = scores.into_iter().collect();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        ranked.truncate(k);
        ranked
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index() -> TextIndex {
        let index = TextIndex::new(true);
        index.add(1, "Decided to keep RocksDB for the local daemon");
        index.add(
            2,
            "SurrealDB HNSW index and RocksDB column families, RocksDB tuning",
        );
        index.add(3, "Lunch plans for Friday");
        index.add(4, "Решихме да ползваме базата SurrealDB");
        index
    }

    fn search(index: &TextIndex, query: &str) -> Vec<u32> {
        let terms = index.query_terms(query);
        index
            .search(&terms, 10, |_| true)
            .into_iter()
            .map(|r| r.0)
            .collect()
    }

    #[test]
    fn ranks_by_bm25_and_filters() {
        let index = index();
        let ranked = search(&index, "rocksdb tuning");
        assert_eq!(ranked.first(), Some(&2), "{ranked:?}");
        assert!(ranked.contains(&1) && !ranked.contains(&3));
        let terms = index.query_terms("rocksdb");
        assert_eq!(index.search(&terms, 10, |id| id == 1).len(), 1);
        assert_eq!(search(&index, "базата"), [4]);
    }

    /// Words on most records are dropped from queries once the memory is
    /// big enough for frequencies to mean something, in any language.
    #[test]
    fn common_words_are_dropped_by_frequency() {
        let index = TextIndex::new(false);
        for i in 0..40 {
            index.add(i, &format!("jsem byl v práci číslo {i}"));
        }
        index.add(40, "jsem viděl kočku");
        assert_eq!(index.query_terms("Viděl jsem kočku?"), ["kočku", "viděl"]);
        assert!(
            index.search(&["kočku".into()], 10, |_| true).is_empty(),
            "no postings"
        );

        let small = TextIndex::new(false);
        small.add(1, "jsem tady");
        small.add(2, "jsem tam");
        assert_eq!(small.query_terms("jsem tady"), ["jsem", "tady"]);
    }

    #[test]
    fn removed_and_cleared_vectors_are_gone() {
        let index = index();
        index.remove(
            2,
            "SurrealDB HNSW index and RocksDB column families, RocksDB tuning",
        );
        assert_eq!(search(&index, "rocksdb"), [1]);
        index.clear();
        assert!(search(&index, "rocksdb").is_empty());
    }
}
