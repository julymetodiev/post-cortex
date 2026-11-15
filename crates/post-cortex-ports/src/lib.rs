// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The ports of post-cortex (docs/ARCHITECTURE.md sections 3 and 5): the
//! traits the memory capabilities depend on and adapters implement, the
//! types those traits speak, and the shared conformance suites.
//!
//! - [`storage`]: one trait per storage capability, bundled by
//!   [`Stores`]; [`error`]: the errors they report.
//! - [`vectors`]: vector metadata and corpora.
//! - [`rerank`]: the reranker port; [`engine`]: the embedding engine port.
//! - [`index`]: the retrieval port, [`SearchIndex`].
//! - [`archive`]: the export file codec, [`ArchiveCodec`].
//! - [`blobs`]: the bytes of images by their SHA-256, [`BlobStore`].
//! - `conformance` (feature `conformance`): the suites every adapter runs.
//!
//! Depends on `post-cortex-core` and no IO crate. Third-party adapters
//! depend on this crate alone.

#![forbid(unsafe_code)]

pub mod archive;
pub mod blobs;
pub mod engine;
pub mod error;
pub mod index;
pub mod kinds;
pub mod rerank;
pub mod storage;
pub mod vectors;

#[cfg(feature = "conformance")]
pub mod conformance;

pub use archive::{ArchiveCodec, ArchiveWriter};
pub use blobs::{Blob, BlobStore};
pub use engine::EmbeddingEngine;
pub use error::PortError;
pub use index::{
    EntrySelect, IndexEntry, IndexFilter, IndexHit, IndexStats, LoadReport, SearchIndex,
};
pub use kinds::{DocumentKinds, KindsError};
pub use rerank::Reranker;
pub use storage::{
    CardStore, CardWrite, CheckpointStore, DismissalStore, DocumentStore, DocumentWrite,
    EventCursors, EventLog, FreshnessStore, InvalidationScope, LatestPage, LatestQuery,
    MessageStore, SessionChanges, SessionCheckpoint, SessionStore, SessionSummary, StaleEntryInfo,
    StoredWorkspace, Stores, VectorStore, WorkspaceStore, WorkspaceUpdate,
};
pub use vectors::{AUTHOR_KEY, Corpus, MESSAGE_CONTENT_TYPE, VectorMetadata};
