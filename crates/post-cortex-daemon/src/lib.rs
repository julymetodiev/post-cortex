// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! HTTP / gRPC / SSE / stdio daemon for post-cortex.
//!
//! Hosts the rmcp Model Context Protocol surface, the tonic gRPC API
//! (single canonical entry point per `post_cortex_core::PostCortexService`
//! method), and ships the `pcx` CLI binary as `[[bin]]`.
//!
//! Status: **Phase 1 stub.** Contents migrate from `src/daemon/` plus
//! `src/bin/pcx*` in Phase 7.

#![deny(missing_docs, rustdoc::broken_intra_doc_links)]
#![forbid(unsafe_code)]
