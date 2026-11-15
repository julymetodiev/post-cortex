// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! [`VectorDB`]: the arena, the catalog of sessions and content ids, and
//! the word statistics (with BM25 postings when enabled), under one lock.
//! Every search is exact: a scan of the filtered rows (all of them, or the
//! sessions' own), split over the rayon pool when it is long. The caller
//! runs these methods off the async runtime.

use std::collections::HashSet;
use std::sync::Arc;

use parking_lot::RwLock;
use post_cortex_ports::{EntrySelect, IndexEntry, IndexFilter, IndexHit, VectorMetadata};
use tracing::warn;

use crate::vector_db::arena::{Arena, Kind};
use crate::vector_db::catalog::Catalog;
use crate::vector_db::common::SearchResult;
use crate::vector_db::similarity::normalize_in_place;
use crate::vector_db::text_index::TextIndex;
use crate::vector_db::types::BulkInsert;

struct State {
    arena: Arena,
    catalog: Catalog,
    text: TextIndex,
}

/// The in-memory vector database (see the module docs).
pub struct VectorDB {
    dimension: usize,
    state: RwLock<State>,
}

impl VectorDB {
    /// An empty database of `dimension`-long vectors, with BM25 postings
    /// over their text.
    pub fn new(dimension: usize) -> Self {
        Self {
            dimension,
            state: RwLock::new(State {
                arena: Arena::new(dimension),
                catalog: Catalog::default(),
                text: TextIndex::default(),
            }),
        }
    }

    /// The length of the vectors the database holds.
    pub fn dimension(&self) -> usize {
        self.dimension
    }

    /// Entries stored.
    pub fn len(&self) -> usize {
        self.state.read().arena.len()
    }

    /// Whether nothing is stored.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Store `entries`, each replacing the entry of its (session, content
    /// id). Vectors of another dimension are refused and counted.
    pub fn upsert<I>(&self, entries: I) -> BulkInsert
    where
        I: IntoIterator<Item = (Vec<f32>, VectorMetadata)>,
    {
        let mut report = BulkInsert::default();
        let mut state = self.state.write();
        let State {
            arena,
            catalog,
            text,
        } = &mut *state;
        for (vector, meta) in entries {
            if vector.len() != self.dimension {
                report.rejected_dimension += 1;
                continue;
            }
            let session = catalog.intern(&meta.source);
            // The entry it replaces goes first, so its slot is reused.
            if let Some(old) = catalog.slot(session, &meta.id)
                && let Some(replaced) = arena.remove(old)
            {
                text.remove(old, &replaced.text);
            }
            let meta = Arc::new(meta);
            let slot = arena.insert(vector, Arc::clone(&meta), session);
            text.add(slot, &meta.text);
            catalog.put(session, &meta.id, slot);
            report.loaded += 1;
        }
        if report.rejected_dimension > 0 {
            warn!(
                "Refused {} vectors whose dimension is not {}",
                report.rejected_dimension, self.dimension
            );
        }
        report
    }

    /// Delete the entries `content_ids` of `session`, each with its chunks
    /// (`<id>#2`, `<id>#3`, ... while there are more). Returns how many
    /// went and the content ids to delete from storage (the given ones and
    /// the chunks found).
    pub fn delete(&self, session: &str, content_ids: &[String]) -> (usize, Vec<String>) {
        let mut state = self.state.write();
        let State {
            arena,
            catalog,
            text,
        } = &mut *state;
        let Some(id) = catalog.find(session) else {
            return (0, content_ids.to_vec());
        };
        let mut removed = 0;
        let mut stored = Vec::with_capacity(content_ids.len());
        for content_id in content_ids {
            stored.push(content_id.clone());
            let mut take = |content: &str| match catalog.release(id, content) {
                Some(slot) => {
                    if let Some(meta) = arena.remove(slot) {
                        text.remove(slot, &meta.text);
                        removed += 1;
                    }
                    true
                }
                None => false,
            };
            take(content_id);
            for n in 2.. {
                let chunk = format!("{content_id}#{n}");
                if !take(&chunk) {
                    break;
                }
                stored.push(chunk);
            }
        }
        (removed, stored)
    }

    /// The stored entries of `session` that `select` names, with their
    /// normalised vectors, in no particular order.
    pub fn entries(&self, session: &str, select: &EntrySelect) -> Vec<IndexEntry> {
        let state = self.state.read();
        let Some(id) = state.catalog.find(session) else {
            return Vec::new();
        };
        let slots: Vec<u32> = match select {
            EntrySelect::All => state.catalog.slots(id).collect(),
            EntrySelect::ContentIds(content_ids) => content_ids
                .iter()
                .flat_map(|content_id| with_chunks(&state.catalog, id, content_id))
                .collect::<HashSet<u32>>()
                .into_iter()
                .collect(),
            EntrySelect::Kinds(kinds) => state
                .catalog
                .slots(id)
                .filter(|&slot| {
                    state
                        .arena
                        .meta(slot)
                        .is_some_and(|m| kinds.contains(&m.content_type))
                })
                .collect(),
        };
        slots
            .into_iter()
            .filter_map(|slot| {
                Some(IndexEntry {
                    vector: state.arena.row(slot)?.to_vec(),
                    meta: VectorMetadata::clone(state.arena.meta(slot)?),
                })
            })
            .collect()
    }

    /// The `k` entries most similar to `query` among those `filter`
    /// covers, best first.
    pub fn nearest(&self, query: &[f32], filter: &IndexFilter, k: usize) -> Vec<IndexHit> {
        let mut query = query.to_vec();
        normalize_in_place(&mut query);
        let state = self.state.read();
        let arena = &state.arena;
        let by_meta = filter.date_range.is_some() || filter.author.is_some();
        let admit = |slot: u32, kind: Kind, _session: u32| {
            kind.in_corpus(filter.corpus)
                && (!by_meta || arena.meta(slot).is_some_and(|m| filter.admits(m)))
        };
        let ranked = match &filter.sessions {
            Some(sessions) => {
                let slots = session_slots(&state.catalog, sessions);
                arena.top_k_of(&slots, &query, k, admit)
            }
            None => arena.top_k(&query, k, admit),
        };
        hits(arena, ranked)
    }

    /// The similarity to `query` of the entries `content_ids` of
    /// `session`, in the order given; ids without an entry are left out.
    pub fn score(&self, query: &[f32], session: &str, content_ids: &[String]) -> Vec<IndexHit> {
        let mut query = query.to_vec();
        normalize_in_place(&mut query);
        let state = self.state.read();
        let Some(id) = state.catalog.find(session) else {
            return Vec::new();
        };
        content_ids
            .iter()
            .filter_map(|content_id| {
                let slot = state.catalog.slot(id, content_id)?;
                Some(IndexHit {
                    similarity: state.arena.similarity(slot, &query)?,
                    meta: Arc::clone(state.arena.meta(slot)?),
                })
            })
            .collect()
    }

    /// The words of `query` worth searching for: deduplicated, without the
    /// ones too common in the stored text to discriminate.
    pub fn query_terms(&self, query: &str) -> Vec<String> {
        self.state.read().text.query_terms(query)
    }

    /// BM25 top-`k` for `terms` (from [`Self::query_terms`]) among the
    /// entries `filter` covers, best first, with similarity 0.
    pub fn lexical(&self, terms: &[String], filter: &IndexFilter, k: usize) -> Vec<IndexHit> {
        let state = self.state.read();
        let arena = &state.arena;
        let sessions: Option<HashSet<u32>> = filter.sessions.as_ref().map(|names| {
            names
                .iter()
                .filter_map(|name| state.catalog.find(name))
                .collect()
        });
        let keep = |slot: u32| {
            arena.kind(slot).in_corpus(filter.corpus)
                && sessions
                    .as_ref()
                    .is_none_or(|ids| arena.session(slot).is_some_and(|id| ids.contains(&id)))
                && arena.meta(slot).is_some_and(|m| filter.admits(m))
        };
        // Equal scores by text, then session and content id: what the
        // content is, not where it is stored.
        let order = |a: &(u32, f32), b: &(u32, f32)| {
            b.1.total_cmp(&a.1)
                .then_with(|| match (arena.meta(a.0), arena.meta(b.0)) {
                    (Some(x), Some(y)) => {
                        (&*x.text, &x.source, &x.id).cmp(&(&*y.text, &y.source, &y.id))
                    }
                    _ => a.0.cmp(&b.0),
                })
        };
        let mut ranked: Vec<(u32, f32)> = state.text.scores(terms, keep).into_iter().collect();
        if k == 0 {
            return Vec::new();
        }
        if ranked.len() > k {
            ranked.select_nth_unstable_by(k - 1, order);
            ranked.truncate(k);
        }
        ranked.sort_by(order);
        ranked
            .into_iter()
            .filter_map(|(slot, _)| {
                Some(IndexHit {
                    similarity: 0.0,
                    meta: Arc::clone(arena.meta(slot)?),
                })
            })
            .collect()
    }

    /// Entries per session.
    pub fn per_session(&self) -> std::collections::HashMap<String, usize> {
        self.state.read().catalog.counts()
    }

    /// Approximate bytes the entries take.
    pub fn memory_bytes(&self) -> usize {
        self.state.read().arena.memory_bytes()
    }
}

/// The slots of `content_id` of session `id` and of its chunks.
fn with_chunks(catalog: &Catalog, id: u32, content_id: &str) -> Vec<u32> {
    let mut slots: Vec<u32> = catalog.slot(id, content_id).into_iter().collect();
    for n in 2.. {
        match catalog.slot(id, &format!("{content_id}#{n}")) {
            Some(slot) => slots.push(slot),
            None => break,
        }
    }
    slots
}

/// The slots of the named sessions (each once).
fn session_slots(catalog: &Catalog, sessions: &[String]) -> Vec<u32> {
    let ids: HashSet<u32> = sessions.iter().filter_map(|s| catalog.find(s)).collect();
    let mut slots: Vec<u32> = ids.into_iter().flat_map(|id| catalog.slots(id)).collect();
    // In arena order: the rows are read front to back.
    slots.sort_unstable();
    slots
}

/// The ranked slots as hits sharing the arena's metadata.
fn hits(arena: &Arena, ranked: Vec<SearchResult>) -> Vec<IndexHit> {
    ranked
        .into_iter()
        .filter_map(|r| {
            Some(IndexHit {
                similarity: r.score,
                meta: Arc::clone(arena.meta(r.slot)?),
            })
        })
        .collect()
}
