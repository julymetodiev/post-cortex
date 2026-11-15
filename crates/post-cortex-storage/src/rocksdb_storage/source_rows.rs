// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The RocksDB row of a source reference (`source_ref:{entry_id}`): the
//! backend's own layout, so a change to the gRPC schema is no change on
//! disk. [`SOURCE_V2`] followed by bincode [`SourceRow`]; rows of the
//! earlier layouts (protobuf, with or without a state prefix) are
//! rewritten when the database is opened (see [`crate::rocksdb_storage::migrate`]).

use crate::port_error::Result;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use post_cortex_core::core::freshness::{BlockScope, FunctionScope, SourceReference, SourceScope};

use crate::freshness::{SourceStatus, StoredState};
use post_cortex_ports::error::corrupt;

/// Marks a source reference row of layout 2.
pub const SOURCE_V2: &[u8] = b"pcx-src-v2\0";

/// A source reference as stored, with its freshness state.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct SourceRow {
    pub entry_id: String,
    pub file_path: String,
    pub content_hash: Vec<u8>,
    pub captured_at_unix: i64,
    pub scope: ScopeRow,
    /// [`SourceStatus::code`].
    pub status: u8,
    /// Session that registered it; `None` for references stored before
    /// sessions were recorded (every invalidation reaches those).
    pub owner: Option<Uuid>,
}

/// [`SourceScope`] as stored.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum ScopeRow {
    WholeFile,
    Function {
        name: String,
        ast_hash: Vec<u8>,
        symbol_type: String,
        imports: Vec<String>,
    },
    Block {
        start_line: u32,
        end_line: u32,
        content_hash: Vec<u8>,
    },
}

impl SourceRow {
    pub fn new(reference: SourceReference, status: SourceStatus, owner: Option<Uuid>) -> Self {
        let scope = match reference.scope {
            SourceScope::WholeFile => ScopeRow::WholeFile,
            SourceScope::Function(f) => ScopeRow::Function {
                name: f.name,
                ast_hash: f.ast_hash,
                symbol_type: f.symbol_type,
                imports: f.imports,
            },
            SourceScope::Block(b) => ScopeRow::Block {
                start_line: b.start_line,
                end_line: b.end_line,
                content_hash: b.content_hash,
            },
        };
        Self {
            entry_id: reference.entry_id,
            file_path: reference.file_path,
            content_hash: reference.content_hash,
            captured_at_unix: reference.captured_at_unix,
            scope,
            status: status.code(),
            owner,
        }
    }

    pub fn reference(&self) -> SourceReference {
        let scope = match &self.scope {
            ScopeRow::WholeFile => SourceScope::WholeFile,
            ScopeRow::Function {
                name,
                ast_hash,
                symbol_type,
                imports,
            } => SourceScope::Function(FunctionScope {
                name: name.clone(),
                ast_hash: ast_hash.clone(),
                symbol_type: symbol_type.clone(),
                imports: imports.clone(),
            }),
            ScopeRow::Block {
                start_line,
                end_line,
                content_hash,
            } => SourceScope::Block(BlockScope {
                start_line: *start_line,
                end_line: *end_line,
                content_hash: content_hash.clone(),
            }),
        };
        SourceReference {
            entry_id: self.entry_id.clone(),
            file_path: self.file_path.clone(),
            content_hash: self.content_hash.clone(),
            captured_at_unix: self.captured_at_unix,
            scope,
        }
    }

    /// Name and type of the function scope, if the reference has one.
    pub fn function(&self) -> Option<(&str, &str)> {
        match &self.scope {
            ScopeRow::Function {
                name, symbol_type, ..
            } => Some((name, symbol_type)),
            _ => None,
        }
    }

    pub fn status(&self) -> Result<SourceStatus> {
        SourceStatus::from_code(i64::from(self.status), &self.entry_id)
    }

    pub fn state(&self) -> Result<StoredState<'_>> {
        let ast_hash = match &self.scope {
            ScopeRow::Function { ast_hash, .. } if !ast_hash.is_empty() => {
                Some(ast_hash.as_slice())
            }
            _ => None,
        };
        Ok(StoredState {
            file_path: &self.file_path,
            content_hash: &self.content_hash,
            ast_hash,
            status: self.status()?,
        })
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut row = SOURCE_V2.to_vec();
        row.extend(bincode::serde::encode_to_vec(
            self,
            bincode::config::standard(),
        )?);
        Ok(row)
    }

    pub fn decode(key: &[u8], row: &[u8]) -> Result<Self> {
        const KIND: &str = "source reference";
        let key = String::from_utf8_lossy(key);
        let body = row
            .strip_prefix(SOURCE_V2)
            .ok_or_else(|| corrupt(KIND, &key, "unknown row layout"))?;
        let (source, read) =
            bincode::serde::decode_from_slice::<Self, _>(body, bincode::config::standard())
                .map_err(|e| corrupt(KIND, &key, e))?;
        if read != body.len() {
            return Err(corrupt(KIND, &key, "trailing bytes").into());
        }
        Ok(source)
    }
}
