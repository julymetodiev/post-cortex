// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Lexical statistics and an optional BM25 index over the vectors' text.
//!
//! Document frequencies are always kept (they are a dictionary, not
//! postings): they decide which query words are too common to search for,
//! for every backend, in place of stopword lists. On LoCoMo dropping words on
//! more than 20% of the records matches a hand-made English stopword list
//! (fast-mode all@50 0.809 vs 0.812) at a third of its query cost, and works
//! in any language. A word's share is of the records written in its script
//! (Latin, Cyrillic, ...), so in a memory that is mostly English the
//! Bulgarian "и" is still found common; in a memory of one script it is the
//! share of all records.
//!
//! Postings are kept only when the database is created with them, for storage
//! backends without full-text search (RocksDB; SurrealDB has a native BM25
//! index). Scoring as SurrealDB's: k1 1.2, b 0.75, statistics over the whole
//! index. Unlike SurrealDB's analyzer it does not fold accents.

use std::collections::{HashMap, HashSet};

use post_cortex_core::text::tokens;

const K1: f32 = 1.2;
const B: f32 = 0.75;
/// Query words on more than this share of the records are not searched for.
const MAX_QUERY_DF_SHARE: f32 = 0.2;
/// ...unless they are on at most this many records, so a small memory keeps
/// every word.
const MIN_PRUNED_DF: u32 = 10;

/// Document frequencies and BM25 postings, keyed by slot.
/// Adding or removing a text costs O(its distinct words).
#[derive(Default)]
pub struct TextIndex {
    /// Term -> number of records containing it.
    df: HashMap<String, u32>,
    /// Records indexed.
    docs: u32,
    /// Script -> number of records with a word in it.
    scripts: HashMap<Script, u32>,
    /// Term -> slot -> term frequency in that slot's text.
    postings: HashMap<String, HashMap<u32, u32>>,
    /// Slot -> number of words in its text.
    lengths: HashMap<u32, u32>,
    total_length: u64,
}

/// The writing system of a word, by its first letter; numbers and the
/// scripts not named are [`Script::Other`], whose share is of all records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Script {
    Latin,
    Greek,
    Cyrillic,
    Armenian,
    Hebrew,
    Arabic,
    Devanagari,
    Thai,
    Georgian,
    Hangul,
    Kana,
    Han,
    Other,
}

fn script(word: &str) -> Script {
    let Some(c) = word.chars().find(|c| c.is_alphabetic()) else {
        return Script::Other;
    };
    match u32::from(c) {
        0x0041..=0x024F | 0x1E00..=0x1EFF => Script::Latin,
        0x0370..=0x03FF | 0x1F00..=0x1FFF => Script::Greek,
        0x0400..=0x052F | 0x1C80..=0x1C8F | 0x2DE0..=0x2DFF | 0xA640..=0xA69F => Script::Cyrillic,
        0x0530..=0x058F => Script::Armenian,
        0x0590..=0x05FF => Script::Hebrew,
        0x0600..=0x06FF | 0x0750..=0x077F | 0x08A0..=0x08FF => Script::Arabic,
        0x0900..=0x097F => Script::Devanagari,
        0x0E00..=0x0E7F => Script::Thai,
        0x10A0..=0x10FF => Script::Georgian,
        0x1100..=0x11FF | 0x3130..=0x318F | 0xAC00..=0xD7AF => Script::Hangul,
        0x3040..=0x30FF => Script::Kana,
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF => Script::Han,
        _ => Script::Other,
    }
}

/// The scripts of a record's words, other than [`Script::Other`].
fn scripts_of(counts: &HashMap<String, u32>) -> HashSet<Script> {
    counts
        .keys()
        .map(|t| script(t))
        .filter(|s| *s != Script::Other)
        .collect()
}

fn term_counts(text: &str) -> HashMap<String, u32> {
    let mut counts = HashMap::new();
    for term in tokens(text) {
        *counts.entry(term).or_default() += 1;
    }
    counts
}

impl TextIndex {
    pub fn add(&mut self, slot: u32, text: &str) {
        let counts = term_counts(text);
        self.docs += 1;
        for script in scripts_of(&counts) {
            *self.scripts.entry(script).or_default() += 1;
        }
        for term in counts.keys() {
            *self.df.entry(term.clone()).or_default() += 1;
        }
        let length: u32 = counts.values().sum();
        for (term, tf) in counts {
            self.postings.entry(term).or_default().insert(slot, tf);
        }
        if let Some(old) = self.lengths.insert(slot, length) {
            self.total_length -= u64::from(old);
        }
        self.total_length += u64::from(length);
    }

    /// Remove `slot`, indexed from `text` (the same text it was added with).
    pub fn remove(&mut self, slot: u32, text: &str) {
        let counts = term_counts(text);
        self.docs = self.docs.saturating_sub(1);
        for script in scripts_of(&counts) {
            if let Some(n) = self.scripts.get_mut(&script) {
                *n -= 1;
                if *n == 0 {
                    self.scripts.remove(&script);
                }
            }
        }
        for term in counts.keys() {
            if let Some(n) = self.df.get_mut(term) {
                *n -= 1;
                if *n == 0 {
                    self.df.remove(term);
                }
            }
            if let Some(list) = self.postings.get_mut(term) {
                list.remove(&slot);
                if list.is_empty() {
                    self.postings.remove(term);
                }
            }
        }
        if let Some(old) = self.lengths.remove(&slot) {
            self.total_length -= u64::from(old);
        }
    }

    /// The words of `query` worth searching for, deduplicated: those on more
    /// than [`MAX_QUERY_DF_SHARE`] of the records written in their script
    /// (and more than [`MIN_PRUNED_DF`]) are dropped.
    pub fn query_terms(&self, query: &str) -> Vec<String> {
        let mut terms = tokens(query);
        terms.sort_unstable();
        terms.dedup();
        terms.retain(|t| {
            let docs = match script(t) {
                Script::Other => self.docs,
                script => self.scripts.get(&script).copied().unwrap_or(0),
            };
            let max_df = ((MAX_QUERY_DF_SHARE * docs as f32) as u32).max(MIN_PRUNED_DF);
            self.df.get(t).copied().unwrap_or(0) <= max_df
        });
        terms
    }

    /// The BM25 score for `terms` of every slot `keep` accepts that has
    /// one of them, in no particular order. Empty without postings.
    pub fn scores(&self, terms: &[String], keep: impl Fn(u32) -> bool) -> HashMap<u32, f32> {
        let mut scores: HashMap<u32, f32> = HashMap::new();
        let docs = self.lengths.len() as f32;
        if terms.is_empty() || docs == 0.0 {
            return scores;
        }
        let avg_length = self.total_length as f32 / docs;
        for term in terms {
            let Some(list) = self.postings.get(term) else {
                continue;
            };
            let df = list.len() as f32;
            let idf = ((docs - df + 0.5) / (df + 0.5) + 1.0).ln();
            for (&slot, &tf) in list {
                if !keep(slot) {
                    continue;
                }
                let length = self.lengths.get(&slot).copied().unwrap_or(0) as f32;
                let tf = tf as f32;
                let norm = tf * (K1 + 1.0) / (tf + K1 * (1.0 - B + B * length / avg_length));
                *scores.entry(slot).or_default() += idf * norm;
            }
        }
        scores
    }
}

#[cfg(test)]
mod tests;
