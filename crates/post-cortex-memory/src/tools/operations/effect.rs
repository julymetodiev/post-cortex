// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What an operation does to memory and which of its fields name sessions
//! and workspaces (ARCHITECTURE section 6): what a gateway in front of many
//! memory instances needs to admit, limit and count a call, read from the
//! table instead of a copy of it.
//!
//! Fields are named as in the operation's `pcx.v1` request message (a
//! nested field as `settings.commit`; an answer's field as in its response
//! message); an operation without an RPC names its MCP arguments or REST
//! body fields, which use the same names. A gateway maps a name to its
//! field number through `post_cortex_proto::PCX_DESCRIPTOR_SET`.

/// A usage counter an operation adds to or gives back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Counter {
    /// Stored records.
    Records,
    /// Captured conversation turns.
    Messages,
    /// Workspaces.
    Workspaces,
}

/// How much a counter moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Count {
    /// One per call.
    One,
    /// One per element of this repeated request field.
    Items(&'static str),
    /// The value of this field of the answer.
    Answer(&'static str),
}

/// A counter and how much it moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tally {
    /// The counter.
    pub counter: Counter,
    /// By how much.
    pub by: Count,
}

/// What makes a search run a reranker: this request field holding one of
/// these values (any case); whether one did, its answer's `answered`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rerank {
    /// The field.
    pub field: &'static str,
    /// The values that rerank.
    pub values: &'static [&'static str],
    /// The value that runs the search without its reranker.
    pub fallback: &'static str,
    /// The boolean field of the answer that says whether a reranker
    /// ordered the hits (one may not be configured, or not allowed).
    pub answered: &'static str,
}

/// What an operation does to memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// Reads only.
    Read,
    /// A search; with `rerank`, the request decides whether a reranker runs.
    Search {
        /// What makes it rerank (`None`: it never does).
        rerank: Option<Rerank>,
    },
    /// Creates or changes memory, adding to `counts`.
    Write {
        /// The counters it adds to.
        counts: &'static [Tally],
        /// A flag field: when false the call writes nothing and is a read
        /// (an import's preview).
        commit: Option<&'static str>,
    },
    /// Removes memory, giving back `counts`.
    Delete {
        /// The counters it gives back to.
        counts: &'static [Tally],
    },
}

/// The fields of a request that name the sessions and workspaces it
/// touches. A request naming none spans the whole memory instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScopeFields {
    /// Fields holding session ids (single or repeated).
    pub sessions: &'static [&'static str],
    /// Fields holding workspace ids (single or repeated).
    pub workspaces: &'static [&'static str],
}
