// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Protobuf + tonic-generated wire types for post-cortex.
//!
//! This crate is intentionally a thin schema package: it owns
//! [`proto/pcx.proto`](../proto/pcx.proto) and re-exports the generated
//! `pcx.v1` module. Downstream gRPC clients can depend on this crate alone
//! without pulling the full `post-cortex-daemon` server runtime.
//!
//! Status: **Phase 1 stub.** The proto + build.rs are migrated in Phase 2
//! of the workspace refactor (see
//! `/Users/julius/.claude/plans/stateful-hugging-hopper.md`).

#![forbid(unsafe_code)]
#![deny(missing_docs, rustdoc::broken_intra_doc_links)]
