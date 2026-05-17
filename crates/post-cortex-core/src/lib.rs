// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Core domain library for post-cortex.
//!
//! Owns the lock-free conversation memory system, semantic search engine,
//! entity graph, storage abstractions, and the canonical
//! `PostCortexService` trait that transports (gRPC, MCP) delegate to.
//!
//! Status: **Phase 1 stub.** Contents migrate from the legacy root crate
//! in Phase 3 (modules `core/`, `storage/`, `session/`, `graph/`,
//! `summary/`, `workspace/`) and Phase 4 (the `services::PostCortexService`
//! trait). See `/Users/julius/.claude/plans/stateful-hugging-hopper.md`.

#![forbid(unsafe_code)]
#![deny(missing_docs, rustdoc::broken_intra_doc_links)]
