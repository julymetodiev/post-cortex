// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Source freshness: where an entry's information came from, and whether
//! that source still says the same.
//!
//! These are the domain types of the freshness port. Transports map their
//! wire messages to them (gRPC `pcx.v1` in the daemon) and storage adapters
//! persist them in their own formats, so neither a wire change nor a storage
//! change reaches the other.

/// Where an entry's information came from: a file, optionally narrowed to a
/// symbol or a line range, with the hashes it had when captured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceReference {
    /// Entry (record) the reference belongs to.
    pub entry_id: String,
    /// File the information came from.
    pub file_path: String,
    /// Hash of the file when the information was captured (blake3).
    pub content_hash: Vec<u8>,
    /// When the information was captured (Unix seconds).
    pub captured_at_unix: i64,
    /// Part of the file the information came from.
    pub scope: SourceScope,
}

impl SourceReference {
    /// The function scope, if the reference has one.
    pub fn function(&self) -> Option<&FunctionScope> {
        match &self.scope {
            SourceScope::Function(function) => Some(function),
            _ => None,
        }
    }
}

/// Part of a file a reference points into.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SourceScope {
    /// The whole file.
    #[default]
    WholeFile,
    /// One symbol (function, struct, ...).
    Function(FunctionScope),
    /// A line range.
    Block(BlockScope),
}

/// A symbol a reference points to.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FunctionScope {
    /// Symbol name.
    pub name: String,
    /// Hash of the symbol's syntax tree; empty when unknown.
    pub ast_hash: Vec<u8>,
    /// Symbol kind as the client names it ("Function", "Struct", ...).
    pub symbol_type: String,
    /// Symbols this symbol depends on.
    pub imports: Vec<String>,
}

/// A line range a reference points to.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BlockScope {
    /// First line.
    pub start_line: u32,
    /// Last line.
    pub end_line: u32,
    /// Hash of the block's text.
    pub content_hash: Vec<u8>,
}

/// Whether an entry's source still says what it said when captured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreshnessStatus {
    /// The source is unchanged.
    Fresh,
    /// The source changed, or the reference was invalidated.
    Stale,
    /// The source no longer exists.
    SourceMissing,
    /// No reference is stored for the entry.
    Unknown,
}

/// The freshness of one entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FreshnessEntry {
    /// Entry that was checked.
    pub entry_id: String,
    /// File of the stored reference; empty when there is none.
    pub file_path: String,
    /// Result of the check.
    pub status: FreshnessStatus,
    /// Stored file hash; empty when there is no reference.
    pub stored_hash: Vec<u8>,
    /// Hash the client sent.
    pub current_hash: Vec<u8>,
}

/// A symbol in a file, as the dependency graph names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolId {
    /// File of the symbol; empty when unknown.
    pub file_path: String,
    /// Symbol name.
    pub symbol_name: String,
    /// Symbol kind ("Function", "Struct", ...).
    pub symbol_type: String,
}

/// What a cascade invalidation marked stale.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CascadeInvalidateReport {
    /// References of the changed symbol itself.
    pub direct_invalidations: u32,
    /// References of symbols that depend on it.
    pub cascade_invalidations: u32,
    /// The dependent symbols, as `{file}::{name}`.
    pub invalidated_symbols: Vec<String>,
}
