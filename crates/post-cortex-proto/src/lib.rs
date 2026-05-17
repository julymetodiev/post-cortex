// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Protobuf + tonic-generated wire types for post-cortex.
//!
//! Single source of truth for the post-cortex gRPC schema. Downstream
//! gRPC clients can depend on this crate alone — they pull `prost`,
//! `prost-types`, and `tonic` (with its default transport features),
//! but **not** the post-cortex-daemon server runtime or rmcp.
//!
//! # Public surface
//!
//! The [`pb`] module re-exports everything generated from
//! `proto/pcx.proto` (package `pcx.v1`). For convenience the most
//! frequently used wire types are also re-exported at the crate root —
//! see the top-level `pub use pb::*` below.
//!
//! # Stability
//!
//! Breaking changes to the proto schema are breaking changes to this
//! crate. SemVer applies normally; consumers should pin a minor version
//! (`= "0.x"`) until 1.0.

#![forbid(unsafe_code)]
// Generated code from prost/tonic does not satisfy the strict doc lints;
// scope the deny to our own modules only.
#![deny(rustdoc::broken_intra_doc_links)]

/// Tonic-generated bindings for the `pcx.v1` proto package.
///
/// Equivalent to `include!(concat!(env!("OUT_DIR"), "/pcx.v1.rs"))` —
/// see `proto/pcx.proto` for the source schema.
#[allow(clippy::all, missing_docs)]
pub mod pb {
    tonic::include_proto!("pcx.v1");
}

// Top-level re-exports of the most commonly-used wire types so the rest of
// the workspace can write `post_cortex_proto::FreshnessEntry` instead of
// `post_cortex_proto::pb::FreshnessEntry`. Matches the names that used to
// live under `crate::daemon::grpc_service::pb` in the legacy single-crate
// layout — this is the migration that unblocks the Phase 3 core split.
pub use pb::{
    CascadeInvalidateReport, FreshnessEntry, FreshnessStatus, SourceReference, SymbolId,
};
