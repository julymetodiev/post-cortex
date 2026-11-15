// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Source reference rows before data version 1: the protobuf
//! `pcx.v1.SourceReference` as of 0.5.0, either bare (registered, owner
//! unknown) or after a state prefix `0x00`, layout `1`, status code, owner
//! (16 bytes, nil = unknown). A protobuf record never starts with `0x00`.
//!
//! The messages below are a frozen copy of that schema, not the live wire
//! types: the rows on disk keep their layout whatever the gRPC schema
//! becomes.

use crate::port_error::Result;
use prost::Message;
use uuid::Uuid;

use post_cortex_core::core::freshness::{BlockScope, FunctionScope, SourceReference, SourceScope};

use crate::freshness::SourceStatus;
use crate::rocksdb_storage::source_rows::SourceRow;
use post_cortex_ports::error::corrupt;

const STATE_MARKER: u8 = 0x00;
const STATE_LAYOUT: u8 = 1;

/// The reference of a pre-version-1 row at `key`.
pub fn decode_source(key: &str, row: &[u8]) -> Result<SourceRow> {
    const KIND: &str = "source reference";
    let (status, owner, proto) = match row {
        [STATE_MARKER, STATE_LAYOUT, status, rest @ ..] if rest.len() >= 16 => {
            let owner = Uuid::from_slice(&rest[..16]).map_err(|e| corrupt(KIND, key, e))?;
            (
                SourceStatus::from_code(i64::from(*status), key)?,
                (!owner.is_nil()).then_some(owner),
                &rest[16..],
            )
        }
        [STATE_MARKER, ..] => return Err(corrupt(KIND, key, "unknown row layout").into()),
        bare => (SourceStatus::Registered, None, bare),
    };
    let reference = WireSourceReference::decode(proto).map_err(|e| corrupt(KIND, key, e))?;
    Ok(SourceRow::new(reference.into(), status, owner))
}

/// `pcx.v1.SourceReference` as of 0.5.0.
#[derive(Clone, PartialEq, Message)]
struct WireSourceReference {
    #[prost(string, tag = "1")]
    file_path: String,
    #[prost(bytes = "vec", tag = "2")]
    content_hash: Vec<u8>,
    #[prost(int64, tag = "3")]
    captured_at_unix: i64,
    #[prost(string, tag = "4")]
    entry_id: String,
    #[prost(message, optional, tag = "5")]
    scope: Option<WireSourceScope>,
}

/// `pcx.v1.SourceScope` as of 0.5.0.
#[derive(Clone, PartialEq, Message)]
struct WireSourceScope {
    #[prost(oneof = "Scope", tags = "1, 2, 3")]
    scope: Option<Scope>,
}

#[derive(Clone, PartialEq, prost::Oneof)]
enum Scope {
    #[prost(bool, tag = "1")]
    WholeFile(bool),
    #[prost(message, tag = "2")]
    Function(WireFunctionScope),
    #[prost(message, tag = "3")]
    Block(WireBlockScope),
}

/// `pcx.v1.FunctionScope` as of 0.5.0.
#[derive(Clone, PartialEq, Message)]
struct WireFunctionScope {
    #[prost(string, tag = "1")]
    name: String,
    #[prost(bytes = "vec", tag = "2")]
    ast_hash: Vec<u8>,
    #[prost(string, tag = "3")]
    symbol_type: String,
    #[prost(string, repeated, tag = "4")]
    imports: Vec<String>,
}

/// `pcx.v1.BlockScope` as of 0.5.0.
#[derive(Clone, PartialEq, Message)]
struct WireBlockScope {
    #[prost(uint32, tag = "1")]
    start_line: u32,
    #[prost(uint32, tag = "2")]
    end_line: u32,
    #[prost(bytes = "vec", tag = "3")]
    content_hash: Vec<u8>,
}

impl From<WireSourceReference> for SourceReference {
    fn from(r: WireSourceReference) -> Self {
        let scope = match r.scope.and_then(|s| s.scope) {
            None | Some(Scope::WholeFile(_)) => SourceScope::WholeFile,
            Some(Scope::Function(f)) => SourceScope::Function(FunctionScope {
                name: f.name,
                ast_hash: f.ast_hash,
                symbol_type: f.symbol_type,
                imports: f.imports,
            }),
            Some(Scope::Block(b)) => SourceScope::Block(BlockScope {
                start_line: b.start_line,
                end_line: b.end_line,
                content_hash: b.content_hash,
            }),
        };
        SourceReference {
            entry_id: r.entry_id,
            file_path: r.file_path,
            content_hash: r.content_hash,
            captured_at_unix: r.captured_at_unix,
            scope,
        }
    }
}
