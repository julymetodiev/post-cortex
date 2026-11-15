// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Pure domain primitives shared by every post-cortex crate.
//!
//! After Phase 3 the orchestrator (`memory_system`, `content_vectorizer`,
//! `query_cache`, `context_assembly`,
//! `scoring`, `performance`) lives in `post-cortex-memory`; the
//! embedding engines + HNSW index live in `post-cortex-embeddings`; the
//! storage backends live in `post-cortex-storage`. What remains here is
//! transport-/IO-free domain logic.

pub mod blob;

/// Project and user cards: small versioned markdown documents put in front
/// of every session.
pub mod card;

/// An agent's conversation: who asked, and how its newest turn stands.
pub mod conversation;

/// Kinds of vectorised content (`ContentKind`).
pub mod content;

/// Context-update data model — `ContextUpdate`, `EntityData`,
/// `EntityRelationship`, `UpdateType`, and supporting types.
pub mod context_update;

/// Documents of modules: versioned data a module keeps in a workspace.
pub mod document;

/// The organisation's bus: the events modules emit, in one ordered log.
pub mod event;

/// Radar dismissals: radar items a workspace set aside.
pub mod dismissal;

/// Source freshness: source references, freshness checks and cascade
/// reports (the freshness port's types).
pub mod freshness;

pub mod message;

/// Structured projection types over a session's update history.
pub mod structured_context;

pub mod wire;
