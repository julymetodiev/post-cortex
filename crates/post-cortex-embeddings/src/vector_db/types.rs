// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The index's own types: tie keys and bulk-insert outcomes.

use serde::{Deserialize, Serialize};

/// A stable key of 64 bits (FNV-1a) of `parts`, each followed by a zero
/// byte so `("ab", "c")` and `("a", "bc")` differ.
fn fnv(parts: &[&str]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    parts
        .iter()
        .flat_map(|part| part.bytes().chain([0]))
        .fold(OFFSET, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(PRIME)
        })
}

/// What equal scores rank by: the text an entry embeds, then its session
/// and content id. A ranking then depends on what the content says, not
/// on the order entries were inserted in or where they are stored.
pub fn tie_key(text: &str, session: &str, content_id: &str) -> (u64, u64) {
    (fnv(&[text]), fnv(&[session, content_id]))
}

/// What a bulk insert stored and what it left out.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BulkInsert {
    /// Vectors stored.
    pub loaded: usize,
    /// Vectors refused because their dimension is not the database's
    /// (stored by another embedding model).
    pub rejected_dimension: usize,
}
