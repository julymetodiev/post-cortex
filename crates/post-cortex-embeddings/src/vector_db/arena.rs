// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The arena: every vector is a row of one contiguous `f32` buffer, scaled
//! to length 1 when it is stored, so a cosine similarity is one dot
//! product. Tag columns beside the rows (kind, session, tie key) let a scan
//! filter without touching the metadata. A removed row's slot is reused by
//! the next insert.

use std::sync::Arc;

use post_cortex_ports::{Corpus, MESSAGE_CONTENT_TYPE, VectorMetadata};
use rayon::prelude::*;

use crate::vector_db::common::{SearchResult, TopK};
use crate::vector_db::similarity::{dot, normalize_in_place};
use crate::vector_db::types::tie_key;

/// Rows one thread scans: a longer scan splits over the rayon pool.
const PARALLEL_ROWS: usize = 16_384;

/// What a slot holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Nothing: the slot is on the free list.
    Free,
    /// A structured record (an update, a chunk of one, an entity).
    Record,
    /// A captured conversation turn.
    Message,
}

impl Kind {
    fn of(meta: &VectorMetadata) -> Self {
        if meta.content_type == MESSAGE_CONTENT_TYPE {
            Self::Message
        } else {
            Self::Record
        }
    }

    /// Whether a row of this kind belongs to `corpus`.
    pub fn in_corpus(self, corpus: Corpus) -> bool {
        match (self, corpus) {
            (Self::Free, _) => false,
            (_, Corpus::All) => true,
            (Self::Record, Corpus::Records) | (Self::Message, Corpus::Messages) => true,
            _ => false,
        }
    }
}

/// The rows and their columns, indexed by slot.
pub struct Arena {
    dimension: usize,
    rows: Vec<f32>,
    kinds: Vec<Kind>,
    sessions: Vec<u32>,
    keys: Vec<(u64, u64)>,
    metas: Vec<Option<Arc<VectorMetadata>>>,
    free: Vec<u32>,
    live: usize,
    /// Bytes of the live entries' strings.
    text_bytes: usize,
}

impl Arena {
    /// An empty arena of `dimension`-long rows.
    pub fn new(dimension: usize) -> Self {
        Self {
            dimension,
            rows: Vec::new(),
            kinds: Vec::new(),
            sessions: Vec::new(),
            keys: Vec::new(),
            metas: Vec::new(),
            free: Vec::new(),
            live: 0,
            text_bytes: 0,
        }
    }

    /// Rows in use.
    pub fn len(&self) -> usize {
        self.live
    }

    /// Store `vector` (of the arena's dimension) with `meta` under the
    /// interned `session`; returns its slot.
    pub fn insert(&mut self, mut vector: Vec<f32>, meta: Arc<VectorMetadata>, session: u32) -> u32 {
        normalize_in_place(&mut vector);
        let kind = Kind::of(&meta);
        let key = tie_key(&meta.text, &meta.source, &meta.id);
        self.text_bytes += size_of(&meta);
        self.live += 1;
        match self.free.pop() {
            Some(slot) => {
                let i = slot as usize;
                self.rows[i * self.dimension..(i + 1) * self.dimension].copy_from_slice(&vector);
                self.kinds[i] = kind;
                self.sessions[i] = session;
                self.keys[i] = key;
                self.metas[i] = Some(meta);
                slot
            }
            None => {
                let slot = self.kinds.len() as u32;
                self.rows.extend_from_slice(&vector);
                self.kinds.push(kind);
                self.sessions.push(session);
                self.keys.push(key);
                self.metas.push(Some(meta));
                slot
            }
        }
    }

    /// Free `slot`; returns what it held.
    pub fn remove(&mut self, slot: u32) -> Option<Arc<VectorMetadata>> {
        let i = slot as usize;
        let meta = self.metas.get_mut(i)?.take()?;
        self.kinds[i] = Kind::Free;
        self.free.push(slot);
        self.live -= 1;
        self.text_bytes -= size_of(&meta);
        Some(meta)
    }

    /// The entry in `slot`.
    pub fn meta(&self, slot: u32) -> Option<&Arc<VectorMetadata>> {
        self.metas.get(slot as usize)?.as_ref()
    }

    /// The kind of `slot`.
    pub fn kind(&self, slot: u32) -> Kind {
        self.kinds.get(slot as usize).copied().unwrap_or(Kind::Free)
    }

    /// The stored (normalised) vector of a live `slot`.
    pub fn row(&self, slot: u32) -> Option<&[f32]> {
        let i = slot as usize;
        (self.kind(slot) != Kind::Free)
            .then(|| &self.rows[i * self.dimension..(i + 1) * self.dimension])
    }

    /// The cosine similarity of a live `slot` to the normalised `query`.
    pub fn similarity(&self, slot: u32, query: &[f32]) -> Option<f32> {
        self.row(slot).map(|row| dot(row, query))
    }

    /// The `k` live rows most similar to the normalised `query` among those
    /// `admit` accepts (by slot, kind and session), best first.
    pub fn top_k<F>(&self, query: &[f32], k: usize, admit: F) -> Vec<SearchResult>
    where
        F: Fn(u32, Kind, u32) -> bool + Sync,
    {
        let slots = self.kinds.len();
        if slots <= PARALLEL_ROWS {
            return self.scan(0..slots, query, k, &admit).into_sorted();
        }
        (0..slots.div_ceil(PARALLEL_ROWS))
            .into_par_iter()
            .map(|chunk| {
                let start = chunk * PARALLEL_ROWS;
                self.scan(start..(start + PARALLEL_ROWS).min(slots), query, k, &admit)
            })
            .reduce(|| TopK::new(k), TopK::merge)
            .into_sorted()
    }

    /// [`Self::top_k`] over the given slots only.
    pub fn top_k_of<F>(&self, slots: &[u32], query: &[f32], k: usize, admit: F) -> Vec<SearchResult>
    where
        F: Fn(u32, Kind, u32) -> bool,
    {
        let mut top = TopK::new(k);
        for &slot in slots {
            self.offer(slot, query, &admit, &mut top);
        }
        top.into_sorted()
    }

    fn scan<F>(&self, slots: std::ops::Range<usize>, query: &[f32], k: usize, admit: &F) -> TopK
    where
        F: Fn(u32, Kind, u32) -> bool,
    {
        let mut top = TopK::new(k);
        for slot in slots {
            self.offer(slot as u32, query, admit, &mut top);
        }
        top
    }

    #[inline]
    fn offer<F>(&self, slot: u32, query: &[f32], admit: &F, top: &mut TopK)
    where
        F: Fn(u32, Kind, u32) -> bool,
    {
        let i = slot as usize;
        let kind = self.kinds[i];
        if kind == Kind::Free || !admit(slot, kind, self.sessions[i]) {
            return;
        }
        let row = &self.rows[i * self.dimension..(i + 1) * self.dimension];
        top.push(SearchResult {
            slot,
            score: dot(row, query),
            key: self.keys[i],
        });
    }

    /// The interned session of a live `slot`.
    pub fn session(&self, slot: u32) -> Option<u32> {
        (self.kind(slot) != Kind::Free).then(|| self.sessions[slot as usize])
    }

    /// Approximate bytes the arena takes.
    pub fn memory_bytes(&self) -> usize {
        let columns = std::mem::size_of::<Kind>()
            + std::mem::size_of::<u32>()
            + std::mem::size_of::<(u64, u64)>()
            + std::mem::size_of::<Option<Arc<VectorMetadata>>>();
        self.rows.capacity() * std::mem::size_of::<f32>()
            + self.kinds.capacity() * columns
            + self.live * std::mem::size_of::<VectorMetadata>()
            + self.text_bytes
    }
}

/// Bytes of the strings an entry holds.
fn size_of(meta: &VectorMetadata) -> usize {
    meta.text.len()
        + meta.id.len()
        + meta.source.len()
        + meta.content_type.len()
        + meta
            .metadata
            .iter()
            .map(|(k, v)| k.len() + v.len())
            .sum::<usize>()
}
