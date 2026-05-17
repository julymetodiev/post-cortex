// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! `post-cortex` — facade meta-crate.
//!
//! Single dependency for projects that want the full post-cortex stack
//! without picking individual crates. Re-exports
//! [`post_cortex_core`](https://docs.rs/post-cortex-core) and (when the
//! corresponding features are enabled)
//! [`post_cortex_mcp`](https://docs.rs/post-cortex-mcp) and
//! [`post_cortex_daemon`](https://docs.rs/post-cortex-daemon).
//!
//! Status: **Phase 1 stub.** Re-exports land in Phase 8 once all source
//! modules have migrated to their target crates. Published under the
//! placeholder name `post-cortex-facade` until Phase 8 swaps the legacy
//! root crate out of the workspace.

#![forbid(unsafe_code)]
#![deny(missing_docs, rustdoc::broken_intra_doc_links)]
