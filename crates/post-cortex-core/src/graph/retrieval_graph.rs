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
//! [`SimpleEntityGraph`](super::entity_graph::SimpleEntityGraph).
//!
//! On LoCoMo the graph raises the share of questions whose evidence is in
//! the top-300 candidate pool from 0.924 to 0.947 (multi-hop 0.798 to
//! 0.848), seeded by the query alone; spaCy NER + noun lemmas, seeded also
//! by a cross-encoder's top 5, gave 0.954.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use rust_stemmers::{Algorithm, Stemmer};

use crate::core::context_update::EntityRelationship;

/// Nodes on more than this share of the records are dropped: they connect
/// everything (speakers, the project name, "today").
const MAX_DF_SHARE: f32 = 0.05;
/// PageRank damping; lower keeps the walk close to the seeds.
const DAMPING: f32 = 0.5;
const ITERATIONS: usize = 30;

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
    /// as `"entity:" + lowercased name` so they never collide with terms.
    nodes: HashMap<String, usize>,
    /// Adjacency over all nodes (records first).
    edges: Vec<Vec<usize>>,
    /// Records each non-record node is on, for seed weights.
    df: Vec<f32>,
}

fn entity_key(name: &str) -> String {
    format!("entity:{}", name.to_lowercase())
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
        for record in records {
            let mut keys = record.terms;
            keys.extend(record.entities.iter().map(|e| entity_key(e)));
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
        let df: Vec<f32> = kept.iter().map(|k| count[k] as f32).collect();

        let mut edges = vec![Vec::new(); n + nodes.len()];
        for (r, keys) in keys_per_record.iter().enumerate() {
            for k in keys {
                if let Some(&node) = nodes.get(k) {
                    edges[r].push(node);
                    edges[node].push(r);
                }
            }
        }
        for rel in relations {
            let (Some(&a), Some(&b)) = (
                nodes.get(&entity_key(&rel.from_entity)),
                nodes.get(&entity_key(&rel.to_entity)),
            ) else {
                continue;
            };
            if a != b {
                edges[a].push(b);
                edges[b].push(a);
            }
        }
        Self {
            records: ids,
            nodes,
            edges,
            df,
        }
    }

    /// Records ranked by personalised PageRank from the query, best first,
    /// at most `limit`. Seeds are the query's terms and the agent entities
    /// it names, each weighted by 1/df. Empty when the query reaches no node.
    pub fn rank(&self, query: &str, limit: usize) -> Vec<R> {
        let n = self.records.len();
        let mut seed = vec![0f32; self.edges.len()];
        let lower = query.to_lowercase();
        let entity_seeds = self
            .nodes
            .iter()
            .filter_map(|(k, &node)| Some((k.strip_prefix("entity:")?, node)))
            .filter(|(name, _)| !name.is_empty() && lower.contains(name))
            .map(|(_, node)| node);
        let term_seeds = terms(query)
            .into_iter()
            .filter_map(|t| self.nodes.get(&t).copied());
        for node in term_seeds.chain(entity_seeds) {
            seed[node] = 1.0 / self.df[node - n];
        }
        let total: f32 = seed.iter().sum();
        if total == 0.0 {
            return Vec::new();
        }
        seed.iter_mut().for_each(|s| *s /= total);

        let mut rank = seed.clone();
        let mut next = vec![0f32; rank.len()];
        for _ in 0..ITERATIONS {
            for (x, s) in next.iter_mut().zip(&seed) {
                *x = (1.0 - DAMPING) * s;
            }
            for (node, neighbours) in self.edges.iter().enumerate() {
                if neighbours.is_empty() {
                    continue;
                }
                let share = DAMPING * rank[node] / neighbours.len() as f32;
                for &m in neighbours {
                    next[m] += share;
                }
            }
            std::mem::swap(&mut rank, &mut next);
        }

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
mod tests {
    use super::*;
    use crate::core::context_update::RelationType;

    fn record(
        id: usize,
        text: &str,
        entities: Vec<&'static str>,
    ) -> (usize, String, Vec<&'static str>) {
        (id, text.to_string(), entities)
    }

    fn graph(
        rows: &[(usize, String, Vec<&'static str>)],
        relations: &[EntityRelationship],
    ) -> RetrievalGraph<usize> {
        RetrievalGraph::build(
            rows.iter().map(|(id, text, entities)| GraphRecord {
                id: *id,
                terms: terms(text),
                entities: entities.iter().map(|e| e.to_string()).collect(),
            }),
            relations,
        )
    }

    /// Filler so that no real node is above the 5% df cut-off, while common
    /// words ("the", "about") are, as in real text.
    fn filler(rows: &mut Vec<(usize, String, Vec<&'static str>)>) {
        rows.extend((100..160).map(|i| {
            record(
                i,
                &format!("the filler record number {i} about the weather"),
                vec![],
            )
        }));
    }

    #[test]
    fn keys_round_trip_sorted_and_deduplicated() {
        let joined = join_keys(["new york", "adopt", "adopt", ""]);
        assert_eq!(joined, "adopt\nnew york");
        assert_eq!(
            split_keys(&joined).collect::<Vec<_>>(),
            ["adopt", "new york"]
        );
        assert_eq!(split_keys("").count(), 0);
    }

    #[test]
    fn terms_are_stemmed_words_and_names() {
        let t = terms("Caroline researched adoption agencies in New York with Melanie.");
        for want in [
            "caroline", "new york", "melanie", "research", "adopt", "agenc",
        ] {
            assert!(t.contains(want), "{want} missing from {t:?}");
        }
        assert!(
            !t.contains("in"),
            "words under three characters are skipped"
        );
    }

    #[test]
    fn terms_work_in_any_script() {
        let t = terms("Přišel jsem do Prahy, viděl jsem kočku.");
        assert!(t.contains("prahy") && t.contains("kočku"), "{t:?}");
        let t = terms("Решихме да ползваме SurrealDB за базата.");
        assert!(
            t.contains("решихме") && t.contains("базата") && t.contains("surrealdb"),
            "{t:?}"
        );
    }

    /// A record sharing no term with the query is reached through a record
    /// that does (the multi-hop case).
    #[test]
    fn reaches_second_hop_records_through_terms() {
        let mut rows = vec![
            record(0, "Caroline adopted a puppy called Biscuit", vec![]),
            record(1, "Biscuit chewed the sofa yesterday", vec![]),
            record(2, "Melanie painted a sunrise over the lake", vec![]),
        ];
        filler(&mut rows);
        let g = graph(&rows, &[]);
        let ranked = g.rank("What did Caroline's puppy chew?", 10);
        assert_eq!(ranked.first(), Some(&0));
        assert!(ranked.contains(&1), "{ranked:?}");
        assert!(!ranked.contains(&2), "{ranked:?}");
        assert!(g.rank("unrelated zebracorn", 10).is_empty());
    }

    /// Agent entities and their relations connect records whose text does
    /// not overlap at all.
    #[test]
    fn agent_entities_and_relations_join_records() {
        let mut rows = vec![
            record(0, "switched the storage layer", vec!["SurrealDB"]),
            record(
                1,
                "index rebuild blocks startup for minutes",
                vec!["HNSW index"],
            ),
            record(2, "unrelated note about lunch", vec![]),
        ];
        filler(&mut rows);
        let relations = [EntityRelationship {
            from_entity: "HNSW index".into(),
            to_entity: "SurrealDB".into(),
            relation_type: RelationType::DependsOn,
            context: String::new(),
        }];
        let ranked = graph(&rows, &relations).rank("what do we know about surrealdb?", 10);
        assert_eq!(ranked.first(), Some(&0), "{ranked:?}");
        assert!(ranked.contains(&1), "related entity's record: {ranked:?}");
        assert!(!ranked.contains(&2));
        // Without the relation the second record is unreachable.
        let ranked = graph(&rows, &[]).rank("what do we know about surrealdb?", 10);
        assert!(!ranked.contains(&1), "{ranked:?}");
    }
}
