// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Retrieval graph: a third candidate source for search, next to vectors
//! and BM25. Records are joined to the terms extracted from their text, to
//! the entities the agent attached to them, and those entities to each
//! other through the session's typed relations. Personalised PageRank from
//! the query's terms and entities reaches records that share no words with
//! the query but share entities with records that do, which multi-hop
//! questions need.
//!
//! Extracted terms live only here: they are every content word of a record,
//! not entities, and are not written to the session's
//! [`SimpleEntityGraph`](crate::graph::entity_graph::SimpleEntityGraph).
//!
//! On LoCoMo the graph raises the share of questions whose evidence is in
//! the top-300 candidate pool from 0.924 to 0.947 (multi-hop 0.798 to
//! 0.848), seeded by the query alone; spaCy NER + noun lemmas, seeded also
//! by a cross-encoder's top 5, gave 0.954.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use rust_stemmers::{Algorithm, Stemmer};

use crate::core::context_update::EntityRelationship;
use crate::graph::analytics::{PageRank, personalised_pagerank};
use crate::graph::entity_graph::{entity_key, name_keys_in, name_words};

/// Nodes on more than this share of the records are dropped: they connect
/// everything (speakers, the project name, "today").
const MAX_DF_SHARE: f32 = 0.05;
/// PageRank damping (lower keeps the walk close to the seeds) and steps.
const PAGE_RANK: PageRank = PageRank {
    damping: 0.5,
    max_iterations: 30,
    tolerance: 0.0,
};

static STEMMER: LazyLock<Stemmer> = LazyLock::new(|| Stemmer::create(Algorithm::English));

/// The terms of `text`: every word of three or more characters (any
/// script), lowercased and stemmed with the English Snowball stemmer, plus
/// each run of capitalised words as one lowercased name ("new york",
/// "melanie"). No stopword list: words on too many records are dropped from
/// the graph by document frequency. The stemmer only changes English
/// suffixes; on other languages it is applied the same way to records and
/// queries, so matches survive (and Cyrillic is left as is).
pub fn terms(text: &str) -> HashSet<String> {
    let words: Vec<&str> = text
        .split(|c: char| !(c.is_alphanumeric() || c == '\''))
        .map(|w| w.trim_matches('\''))
        .filter(|w| !w.is_empty())
        .collect();
    let mut out = HashSet::new();
    let mut run: Vec<&str> = Vec::new();
    for w in words.iter().map(Some).chain([None]) {
        if let Some(w) = w.filter(|w| w.starts_with(char::is_uppercase)) {
            run.push(w);
        } else if !run.is_empty() {
            let name = run.join(" ").to_lowercase();
            if name.chars().count() > 2 {
                out.insert(name);
            }
            run.clear();
        }
    }
    for w in words {
        let lower = w.to_lowercase();
        if lower.chars().count() > 2 {
            out.insert(STEMMER.stem(&lower).into_owned());
        }
    }
    out
}

/// Metadata key under which a record's [`terms`] are stored with its
/// embedding, joined by [`join_keys`].
pub const TERMS_KEY: &str = "terms";
/// Metadata key under which the names of the entities the agent attached to
/// a record are stored with its embedding, joined by [`join_keys`].
pub const ENTITIES_KEY: &str = "entities";

/// Join terms or entity names for storage in an embedding's metadata (one
/// per line, sorted so the stored value is stable).
pub fn join_keys<'a>(keys: impl IntoIterator<Item = &'a str>) -> String {
    let mut keys: Vec<&str> = keys.into_iter().filter(|k| !k.is_empty()).collect();
    keys.sort_unstable();
    keys.dedup();
    keys.join("\n")
}

/// Inverse of [`join_keys`].
pub fn split_keys(joined: &str) -> impl Iterator<Item = &str> {
    joined.split('\n').filter(|k| !k.is_empty())
}

/// A record as the graph sees it.
pub struct GraphRecord<R> {
    /// Caller's id for the record (e.g. the update id)
    pub id: R,
    /// The record's [`terms`]
    pub terms: HashSet<String>,
    /// Names of the entities the agent attached to the record
    pub entities: Vec<String>,
}

/// Bipartite-plus graph over a session: records on one side; extracted
/// terms and agent entities on the other; entity-entity relation edges.
pub struct RetrievalGraph<R> {
    records: Vec<R>,
    /// Term or entity -> node index (after the records). Entities are keyed
    /// as `"entity:"` + their resolution key ([`entity_key`]), so they never
    /// collide with terms and spellings of one entity meet.
    nodes: HashMap<String, usize>,
    /// Weighted adjacency over all nodes (records first): record-key edges
    /// weigh 1, relation edges their type's
    /// [`RelationType::weight`](crate::core::context_update::RelationType::weight).
    edges: Vec<Vec<(usize, f64)>>,
    /// Total edge weight per node.
    strength: Vec<f64>,
    /// Records each non-record node is on, for seed weights.
    df: Vec<f64>,
    /// Most words in an entity name of the graph, for seeding.
    max_entity_words: usize,
}

fn entity_node_key(name: &str) -> String {
    format!("entity:{}", entity_key(name))
}

impl<R: Clone> RetrievalGraph<R> {
    /// Build from the session's records and the relations between its
    /// entities.
    pub fn build(
        records: impl IntoIterator<Item = GraphRecord<R>>,
        relations: &[EntityRelationship],
    ) -> Self {
        let mut ids = Vec::new();
        let mut keys_per_record: Vec<HashSet<String>> = Vec::new();
        let mut max_entity_words = 0;
        for record in records {
            let mut keys = record.terms;
            for entity in &record.entities {
                max_entity_words = max_entity_words.max(name_words(entity));
                keys.insert(entity_node_key(entity));
            }
            ids.push(record.id);
            keys_per_record.push(keys);
        }
        let n = ids.len();

        let mut count: HashMap<&str, usize> = HashMap::new();
        for keys in &keys_per_record {
            for k in keys {
                *count.entry(k.as_str()).or_default() += 1;
            }
        }
        let max_df = ((MAX_DF_SHARE * n as f32) as usize).max(2);
        let mut kept: Vec<&str> = count
            .iter()
            .filter(|&(_, &c)| c <= max_df)
            .map(|(&k, _)| k)
            .collect();
        kept.sort_unstable();
        let nodes: HashMap<String, usize> = kept
            .iter()
            .enumerate()
            .map(|(i, &k)| (k.to_string(), n + i))
            .collect();
        let df: Vec<f64> = kept.iter().map(|k| count[k] as f64).collect();

        let mut edges = vec![Vec::new(); n + nodes.len()];
        for (r, keys) in keys_per_record.iter().enumerate() {
            for k in keys {
                if let Some(&node) = nodes.get(k) {
                    edges[r].push((node, 1.0));
                    edges[node].push((r, 1.0));
                }
            }
        }
        for rel in relations {
            let (Some(&a), Some(&b)) = (
                nodes.get(&entity_node_key(&rel.from_entity)),
                nodes.get(&entity_node_key(&rel.to_entity)),
            ) else {
                continue;
            };
            if a != b {
                let w = f64::from(rel.relation_type.weight());
                edges[a].push((b, w));
                edges[b].push((a, w));
            }
        }
        let strength = edges
            .iter()
            .map(|e| e.iter().map(|(_, w)| w).sum())
            .collect();
        Self {
            records: ids,
            nodes,
            edges,
            strength,
            df,
            max_entity_words,
        }
    }

    /// Records ranked by personalised PageRank from the query, best first,
    /// at most `limit`. Seeds are the query's terms and the agent entities
    /// it names (whole words, see [`name_keys_in`]), each weighted by 1/df.
    /// Empty when the query reaches no node.
    pub fn rank(&self, query: &str, limit: usize) -> Vec<R> {
        let n = self.records.len();
        let mut seed = vec![0f64; self.edges.len()];
        let entity_seeds = name_keys_in(query, self.max_entity_words)
            .into_iter()
            .filter_map(|key| self.nodes.get(&format!("entity:{key}")).copied());
        let term_seeds = terms(query)
            .into_iter()
            .filter_map(|t| self.nodes.get(&t).copied());
        for node in term_seeds.chain(entity_seeds) {
            seed[node] = 1.0 / self.df[node - n];
        }
        let total: f64 = seed.iter().sum();
        if total == 0.0 {
            return Vec::new();
        }
        seed.iter_mut().for_each(|s| *s /= total);

        let rank = personalised_pagerank(&self.edges, &self.strength, &seed, PAGE_RANK);
        let mut order: Vec<usize> = (0..n).filter(|&r| rank[r] > 0.0).collect();
        order.sort_by(|&a, &b| rank[b].total_cmp(&rank[a]));
        order.truncate(limit);
        order.into_iter().map(|r| self.records[r].clone()).collect()
    }

    /// Number of records in the graph.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Whether the graph has no records.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

#[cfg(test)]
mod tests;
