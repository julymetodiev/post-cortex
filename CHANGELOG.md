# Changelog

All notable changes to Post-Cortex will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.21] - 2026-02-26

### Added

- **Agent auto-installation in `pcx setup`**: Setup wizard now installs post-cortex agent definitions to `~/.claude/agents/post-cortex-agents/` (context-builder, search-specialist, knowledge-analyst, memory-curator). Checks for existing files before writing.

### Changed

- **Removed `docs/examples/`**: All templates are now embedded in the binary via `src/bin/templates/` and installed by `pcx setup`. Eliminates template drift between docs and source.
- **Trimmed README.md**: Condensed from ~340 to ~120 lines. Removed verbose MCP tool examples, storage backend details, and benchmarks. Added `pcx setup` to Quick Start.
- **Rewrote USAGE_GUIDE.md**: Quick Setup now points to `pcx setup` instead of manual MCP calls and file copying. Removed all `docs/examples/` references.

## [0.1.20] - 2026-02-26

### Added

- **`pcx setup` interactive wizard**: New command that automates project initialization for Claude Code integration
  - Creates sessions and workspaces via the running daemon
  - Generates `.claude/` configuration files (CLAUDE.md, settings.json, hooks)
  - Supports non-interactive mode with `--defaults` flag
  - Templates for CLAUDE.md, settings.json, and context re-injection hooks
  - `dialoguer` dependency for interactive CLI prompts

### Changed

- **Project configuration refactored**: Moved agent definitions and hook scripts to `docs/examples/`
  - Example configurations now live in `docs/examples/` (CLAUDE.md, settings.json, hooks, agents)
  - `pcx setup` generates project-specific configs from built-in templates instead of manual copying
  - Simplified CLAUDE.md and USAGE_GUIDE.md to reference template-based setup

### Fixed

- **Daemon workspace state sync**: In-memory workspace state now syncs after API changes (create/delete), preventing stale data in subsequent requests

## [0.1.19] - 2026-02-26

### Added

- **NER engine pre-loading on daemon startup**: DistilBERT NER model is now loaded eagerly via `tokio::spawn` during daemon init, ensuring entity extraction uses the ML model instead of the regex/pattern-matching fallback
- **`pcx rebuild-entities` CLI command**: Rebuilds the entity graph for a session (or all sessions with `--all`) by replaying all context updates through the NER engine, replacing noisy pattern-matched entities with precise DistilBERT-extracted ones
- **`clear_session_entities` on Storage trait**: Deletes all entities and relationships for a session from both RocksDB and SurrealDB before saving the rebuilt graph, preventing stale entities from persisting after rebuild

### Fixed

- **NER engine was never loaded**: `preload_ner_engine()` was defined but never called — all entity extraction silently used the pattern-matching fallback, producing ~4x more noisy entities (e.g., "access", "Arc", "HashMap" stored as entities)
- **Stop hook not enforcing Rule 2**: Rewrote hook reason message from diagnostic to actionable format so Claude auto-fixes compliance violations
- **SurrealDB entity cleanup on rebuild**: `save_session` uses UPSERT which doesn't remove old entities — added explicit DELETE of all entity + relationship tables before saving rebuilt graph

## [0.1.18] - 2026-02-26

### Changed

- **Non-blocking context updates (fire-and-forget persistence)**:
  - `add_incremental_update` no longer blocks on storage writes — data is committed in-memory via ArcSwap CAS, then persisted asynchronously
  - New `PersistSessionAndUpdate` message variant in StorageActor with `persist_session_and_update_nowait()` on the handle
  - Eliminates two sequential actor round-trips + `spawn_blocking` calls from the response path

- **Combined RocksDB write (`save_session_with_updates`)**:
  - New `save_session_with_updates` method on `Storage` trait with default implementation
  - RocksDB backend: single `WriteBatch` + single `spawn_blocking` for both session blob and update records (was 2 separate writes)

- **Background entity graph extraction**:
  - New `add_incremental_update_fast()` on `ActiveSession` — skips `update_entity_graph` (up to 5s timeout)
  - Entity graph update runs in a background `tokio::spawn` task after CAS success
  - New `apply_entity_graph_update()` for the background path
  - Background CAS failure is benign — next update catches up

- **Batched SurrealDB operations**:
  - `save_session` now builds all upserts (session metadata + context updates + entities + relationships) into a single multi-statement SurrealQL query (1 network round-trip instead of N)
  - `batch_save_updates` uses single multi-statement query instead of individual upserts per update
  - Estimated 50-100x reduction in `save_session` latency for remote SurrealDB backends

### Fixed

- SurrealDB `save_session` performance: was doing N+1 individual WebSocket round-trips (1 session + N updates + N entities + N relationships); now 1 round-trip

## [0.1.17] - 2026-02-26

### Added

- **Temporal Decay (Recency Bias) for Semantic Search** (PR #11, thanks to neiromaster):
  - New `recency_bias` parameter on all semantic search scopes (session, workspace, global)
  - Exponential decay formula: `base_score * e^(-lambda * days / 365)`
  - Values: 0.0 (disabled) → 0.1–0.5 (soft bias) → 1.0+ (aggressive bias)
  - Input validation: range [0.0, 10.0], rejects NaN/Infinity/negative values
  - Cache key includes `recency_bias` to prevent collisions between different bias values
  - Date range filtering integrates correctly with recency bias
  - Works across all three search scopes and both storage backends
  - Performance metrics: lock-free atomic counters for decay calculation tracking
  - Criterion microbenchmarks for regression testing (~317 ps per decay calculation)

- **MCP Dry-Run Mode and Error Recovery** (PR #10, thanks to neiromaster):
  - `dry_run` parameter on `update_conversation_context` to validate without persisting
  - Detailed preview of what would be stored
  - `generate_recovery_suggestions()` analyzes error patterns and provides actionable hints
  - Recovery suggestions automatically included in all MCP error responses

- **ScoreAdjuster trait for pluggable scoring strategies**: New `src/core/scoring.rs` module
  - `TemporalDecayAdjuster` encapsulates decay logic
  - `CompositeScoreAdjuster` for composing multiple adjusters (Strategy Pattern)
  - Easy to extend with custom adjusters (popularity, quality, etc.)

- **Batch write methods for graph data and embeddings**: RocksDB WriteBatch for saving multiple entities, relationships, and embeddings in a single `spawn_blocking` call instead of N individual writes

- **Docker Compose for SurrealDB and Surrealist**: Development environment setup

- **Claude Code hooks and agent configuration**:
  - Compact reinject hook to restore PCX context after compaction
  - Stop hook for rule compliance verification
  - UserPromptSubmit reminder hook
  - Post-Cortex agent documentation and usage guide

- **Multisession semantic search support**

- **Workspace scope and new interaction types**: `requirement_added` and `concept_defined` for better knowledge capture

### Changed

- **Removed "LockFree" prefix from all type names**: Consolidated indirection layers by inlining implementations into canonical modules
  - Deleted: `lockfree_embeddings.rs`, `lockfree_query_cache.rs`, `lockfree_vector_db.rs`, `embeddings_compat.rs`, `query_cache_compat.rs`
  - Renamed files: `lockfree_cache.rs` → `cache.rs`, `lockfree_memory_system.rs` → `memory_system.rs`, `lockfree_performance.rs` → `performance.rs`
  - Renamed types: `LockFreeConversationMemorySystem` → `ConversationMemorySystem`, `LockFreeCache` → `Cache`, `LockFreeVectorDB` → `VectorDB`, `LockFreeWorkspaceManager` → `WorkspaceManager`, `LockFreeNEREngine` → `NEREngine`, `LockFreeEntityGraph` → `EntityGraph`, `LockFreeDaemonServer` → `DaemonServer`, `LockFreePerformanceMonitor` → `PerformanceMonitor`

- **SearchOptions struct**: Consolidated search parameters to eliminate method proliferation (`_with_recency` wrappers removed, single method per scope)

- **Bumped surrealdb** from 3.0.0-beta to 3.0.0 stable

### Fixed

- **O(n²) performance bottleneck in workspace search**: Replaced N sequential vector DB searches with single `search_with_filter()` call (10 sessions: 500ms → 50ms, 100 sessions: 5s → 50ms)
- **Vectorizer metrics sharing across clones**: Wrapped AtomicU64 metrics in Arc to aggregate across instances
- **Inconsistent response formats**: All search scopes now return consistent JSON with results array
- **Date range + recency_bias integration**: Date range parsing now occurs before recency_bias check
- **Cache key collision**: `recency_bias` included in cache key to prevent stale results
- **recency_bias CLI parity**: Added parameter extraction in CLI handlers for human users
- **Dry-run session existence validation**: Extracted `check_session_exists_for_dry_run()` helper
- **VALID_INTERACTION_TYPES constant**: Extracted for reuse across validation modules

### Improved

- **Lock-free memory system optimizations**:
  - Per-operation counters (load/save/delete) in StorageActor
  - Reduced CAS retry limit from 100 to 20 with exponential backoff
  - Optimized `search_sessions` to prefer in-memory cache over storage
  - Fast-path exact key lookup in entity graph search
  - Batch insertion for HNSW index rebuild with progress logging
  - Replaced DEBUG-level `info!`/`eprintln!` with proper `debug!` tracing

- **Code quality refactoring**:
  - Extracted `format_search_results()` helper (~80 lines deduplication)
  - Unified search logic to remove conditional branching (~60 lines removed)
  - Test fixture builder pattern in `tests/common/mod.rs`
  - `parse_date_range()` helper eliminates ~75 lines of duplicated parsing

## [0.1.16] - 2026-01-16

### Added

- **HNSW Index for RocksDB**: O(log n) vector search now available in RocksDB backend
  - Hybrid architecture: in-memory HNSW index + RocksDB persistence
  - On startup, embeddings are loaded from disk to rebuild the HNSW index
  - Full feature parity with SurrealDB vector search performance

- **Graph Storage for RocksDB**: Persistent entity and relationship storage
  - `StoredEntity` and `StoredRelationship` structs for RocksDB persistence
  - Full `GraphStorage` trait implementation (upsert_entity, get_entity, create_relationship, etc.)
  - BFS-based `find_shortest_path` and `get_entity_network` implementations
  - Embedding dimension validation (384 dimensions) matches SurrealDB

- **MCP Type Coercion**: Better agent interaction with flexible parameter types
  - Automatic type coercion for string/number/boolean parameters
  - Structured error responses for invalid inputs
  - Integration tests for type coercion scenarios

- **MCP Input Schema**: Added `input_schema` field to tool definitions for better client compatibility

- **MCP Validation Layer**: Custom validation for business logic rules (PR #7) (thanks to neiromaster)
  - `validate_session_id()`, `validate_workspace_id()` - UUID format validation
  - `validate_interaction_type()` - Enum validation (6 valid types)
  - `validate_scope()` - Semantic search scope validation
  - `validate_session_action()`, `validate_workspace_action()` - Action validation
  - `validate_session_role()` - Workspace membership roles (4 roles)
  - `validate_limits()` - Numeric bounds checking
  - Enhanced parameter descriptions with examples and valid values
  - 38 new tests (18 unit + 20 integration)

### Fixed

- **RocksDB Session Serialization**: Changed from bincode to serde_json
  - bincode doesn't support `deserialize_any` required by EdgeData custom deserializer
  - Fixes "Serde(AnyNotSupported)" error when loading large sessions
  - Note: Old RocksDB data (bincode format) must be deleted and re-imported

- **UTF-8 Safe String Truncation**: Prevent panic on multi-byte character boundaries
  - Safe truncation for Bulgarian, Chinese, emoji, and other multi-byte characters

- **Integration Test Race Conditions**: Added `serial_test` for reliable parallel test execution (thanks to neiromaster)
  - Tests no longer flake due to shared resource contention

- **Session ID Format Consistency**: Consistent UUID formatting across all MCP responses (thanks to neiromaster)

- **Full UUIDs in Search Output**: `semantic_search_global` now displays complete UUIDs instead of truncated

### Improved

- **Storage Backend Feature Parity**: Both RocksDB and SurrealDB now support identical features
  - Vector Search: HNSW O(log n) for both
  - Graph Storage: Persistent for both
  - Embedding Validation: 384 dimensions for both

- **Storage Code Organization**: Reorganized imports and suppressed unused mut warnings

## [0.1.15] - 2025-12-22

### Fixed

- **MCP Client Compatibility**: Strip `$schema` from tool input schemas
  - Many MCP clients (Cursor, Windsurf, Continue.dev) don't support JSON Schema draft/2020-12
  - Replaced `#[tool_handler]` macro with manual `list_tools` implementation
  - Removes `$schema` field from all tool schemas and nested `$defs`
  - Error "no schema with key or ref 'https://json-schema.org/draft/2020-12/schema'" is now resolved

## [0.1.14] - 2025-12-21

### Changed

- **RMCP Transport Upgrade**: Migrated from SSE to Streamable HTTP transport
  - Updated `rmcp` to 0.12.0 with `transport-streamable-http-server` feature
  - Replaced `SseServer` with `StreamableHttpService` and `LocalSessionManager`
  - Unified endpoint: `/mcp` replaces `/sse` + `/message`
  - MCP client config: `"type": "http"` with `"url": "http://localhost:3737/mcp"`

- **stdio_proxy Lock-Free Rewrite**: Updated to use Streamable HTTP protocol
  - Replaced `RwLock` with `ArcSwap` for session ID storage
  - Session ID extracted from `Mcp-Session-Id` response header
  - Proper `Accept: application/json, text/event-stream` headers

- **bincode Downgrade**: 3.0.0 → 2.0.1 with `serde` feature
  - bincode 3.0.0 contains intentional `compile_error!` (maintainer protest after doxxing incident)
  - Version 2.0.1 is stable and fully functional

### Fixed

- **Documentation Updates**: Updated all MCP endpoint references
  - README.md: HTTP transport configuration
  - DAEMON_MODE.md: Streamable HTTP examples
  - pcx status: Shows `/mcp` and `/health` endpoints

## [0.1.13] - 2025-12-21

### Added

- **Graph-RAG Enrichment**: Semantic search results now include structural insights from the entity graph
  - `[System Knowledge Map]` shows entity relationships in search results
  - `[Structural Insight]` displays paths between top result entities
  - Automatic entity extraction from search queries
  - Direct graph access for O(1) entity lookups (no async message passing)

- **HNSW Vector Index**: Native approximate nearest neighbor search in SurrealDB
  - `idx_embedding_hnsw` - 384-dimensional COSINE distance index
  - O(log n) KNN queries instead of O(n) full table scan
  - Post-filtering for session/content_type scoped searches

- **Composite Database Indexes**: Optimized ORDER BY queries
  - `idx_update_session_time` - (session_id, timestamp) for context_update
  - `idx_entity_session_importance` - (session_id, importance_score) for entity

- **Explicit Entity/Relationship Parsing**: MCP tool now parses entities from content
  - `entities` field: comma/space separated entity names
  - `relationships` field: "A RELATION B" format (DEPENDS_ON, IMPLEMENTS, CAUSES, etc.)

### Improved

- **Vector Search Performance**: Replaced Rust-side cosine similarity with SurrealDB native KNN
  - `search()` - uses HNSW index directly
  - `search_in_session()` - HNSW with post-filtering (5x overfetch)
  - `search_by_content_type()` - HNSW with post-filtering

- **Case-Insensitive Entity Lookup**: Graph queries now support case-insensitive matching
  - `find_related_entities()` - O(1) exact match with O(n) fallback
  - `find_related_entities_by_type()` - case-insensitive entity names
  - `find_shortest_path()` - case-insensitive from/to nodes

### Fixed

- **Relationship Noise Cleanup**: Removed auto-generated "Co-mentioned in:" relationships
  - Deleted `auto_generate_relationships()` function
  - Deleted `infer_relationship_type()` function
  - Only explicit relationships from MCP content are now stored

- **RUST_LOG Environment Variable**: Now properly respects RUST_LOG settings
  - Uses `EnvFilter::try_from_default_env()` with "info" fallback
  - Fixed duplicate directive issue

- **Verbose Logging**: Reduced noise from `add_incremental_update` instrument

## [0.1.12] - 2025-12-20

### Added

- **Embeddings Export/Import**: Full backup and restore of semantic search embeddings
  - Export format v1.2.0 includes embeddings array with vectors, text, and metadata
  - `ExportedEmbedding` struct: content_id, session_id, vector (384-dim), text, content_type, timestamp
  - Backwards-compatible import (v1.0.0, v1.1.0, v1.2.0 formats supported)
  - JSON Schema `schemas/export/v1.2.0.json` with embeddings definition

### Fixed

- **vectorize-all with SurrealDB**: Now respects `storage_backend` configuration
  - Previously always used RocksDB regardless of daemon.toml settings
  - Now properly connects to SurrealDB when configured

## [0.1.11] - 2025-12-20

### Added

- **SurrealDB Storage Backend**: Native graph database support as alternative to RocksDB
  - Native graph storage for entities and relationships using SurrealDB RELATE
  - Normalized context storage with separate tables for sessions, updates, entities
  - Persistent embeddings storage with vector search capabilities
  - Configurable via `daemon.toml` with `storage_backend = "surrealdb"`
  - Namespace and database configuration (`surrealdb_namespace`, `surrealdb_database`)

- **Export/Import for SurrealDB**: Full data portability between storage backends
  - `pcx export` now works with SurrealDB backend
  - `pcx import` imports directly to configured storage backend
  - Backwards-compatible import from v1.0.0 format (EdgeData deserialization)
  - Auto-generated export filenames: `export-YYYY-MM-DD_HH-MM-SS.json.gz`

- **Export Schema Versioning**: JSON Schema definitions for validation
  - `schemas/export/v1.0.0.json` - Original format (RelationType strings)
  - `schemas/export/v1.1.0.json` - New format (EdgeData objects with context)

- **New MCP Interaction Types**: Extended context update types
  - `requirement_added` - Track new requirements
  - `concept_defined` - Document concept definitions

### Improved

- **CLI Storage Backend Support**: All CLI commands respect `storage_backend` config
  - `pcx session list/create/delete` uses configured backend
  - `pcx workspace list/create/delete/attach` uses configured backend
  - Works both with running daemon (HTTP API) and direct storage access

- **Dual Logging for Daemon**: Logs to both file and stderr in foreground mode

### Fixed

- **Export without output path**: Now auto-generates timestamped filename instead of error

## [0.1.10] - 2025-12-14

### Added

- **Data Export/Import**: Full backup and restore functionality for Post-Cortex data
  - `pcx export` - Export sessions, workspaces, updates, and checkpoints to JSON
  - `pcx import` - Import data with selective filtering and conflict handling
  - **Compression support**: gzip (`.json.gz`) and zstd (`.json.zst`) with auto-detection
  - **Selective export**: Export specific sessions (`--session`) or workspaces (`--workspace`)
  - **Import options**: `--skip-existing`, `--overwrite`, `--list` for preview
  - **Versioned format**: Format version 1.0.0 with forward compatibility support
  - **Overwrite protection**: Prompts before overwriting existing files (`--force` to skip)

### Improved

- **CLI help system**: Added detailed `--help` with examples for all commands and subcommands
- **Storage layer**: Added `load_session_updates` and `list_checkpoints` methods to RocksDB storage

## [0.1.9] - 2025-12-11

### Changed

- **MCP Tools Consolidation**: Reduced from 24 tools to 6 consolidated tools for simpler AI integration
  - `session` - Merges create_session + list_sessions via action parameter
  - `update_conversation_context` - Supports bulk mode via optional `updates` array
  - `semantic_search` - Unified search with `scope` parameter (session/workspace/global)
  - `get_structured_summary` - Extended with `include` parameter for selective sections
  - `query_conversation_context` - Added `entity_importance` and `entity_network` query types
  - `manage_workspace` - Merges 6 workspace tools via action parameter

### Fixed

- **RocksDB prefix iteration for short prefixes**: Fixed `prefix_iterator()` not working for prefixes shorter than 16 bytes (the prefix_extractor size). Session list, workspace list, and delete operations now use `iterator(IteratorMode::From)` instead, correctly handling "session:" (8 bytes), "workspace:" (10 bytes), and "ws_session:" (11 bytes) prefixes

### Improved

- **README documentation**: Cleaner documentation with concise 6-tool API examples
- **SummaryGenerator refactoring**: Converted to use associated functions for better code organization
- **Axum route params**: Updated to use bracket syntax for path parameters

## [0.1.8] - 2025-12-09

### Added

- **Lock-free memory pool statistics**: Added hit/miss counters and pool monitoring for performance diagnostics
- **Session-specific cache invalidation**: New method to invalidate cache entries for a single session instead of clearing entire cache
- **Text similarity computation**: Implemented `compute_text_similarity` in ContentVectorizer for accurate cosine similarity calculations between text embeddings

### Changed

- **Copy-on-Write semantics for ActiveSession**: Wrapped large session fields in `Arc` to enable cheap cloning with `Arc::make_mut()` for efficient updates
- **Logical timestamps in cache**: Replaced `SystemTime` calls with Lamport clock to eliminate syscall overhead (~20-100ns per call) in hot paths
- **Memory pool concurrency**: Implemented proper compare-and-swap loops for atomic operations in memory pool and batch size updates

### Fixed

- **Concurrent entry point race conditions**: Fixed race conditions in search mode with proper ordering traits for `SearchResult`
- **Statistics underflow**: Added saturating subtraction to prevent underflow on double-remove operations
- **NER engine safety**: Changed cache key from hash to full text to avoid collisions, added bounded cache with eviction, proper memory ordering for thread synchronization, and safe UTF-8 slicing
- **Stale vectorized_update_ids after daemon restart**: Added verification to clear stale IDs when vector embeddings are missing in-memory after restart
- **Semantic search similarity filtering**: Applied similarity threshold filtering consistently across all search functions with improved Unicode support in content truncation

### Improved

- **Pool size limits**: Added limits to prevent unbounded memory pool growth
- **Tensor type conversions**: Optimized for BERT compatibility
- **StructuredContext validation**: Enhanced with better documentation and field name handling in MCP tools

## [0.1.7] - 2025-11-25

### Added

- **REST API for CLI commands**: Added HTTP endpoints to daemon for CLI administration when daemon is running
  - `/health` - Health check endpoint
  - `/api/sessions` - GET/POST for session listing and creation
  - `/api/sessions/{id}` - DELETE for session deletion
  - `/api/workspaces` - GET/POST for workspace listing and creation
  - `/api/workspaces/{id}` - DELETE for workspace deletion
  - `/api/workspaces/{workspace_id}/sessions/{session_id}` - POST for attaching sessions

### Fixed

- **CLI RocksDB lock conflict**: CLI commands now detect running daemon and route through HTTP API instead of direct RocksDB access, preventing "Resource temporarily unavailable" lock errors
- **Stale workspace session count**: Workspace listing now filters deleted sessions, showing accurate count of existing sessions only

### Changed

- **CLI architecture**: Added `DaemonClient` HTTP client and `is_daemon_running()` detection for seamless daemon/direct mode switching

## [0.1.6] - 2025-11-25

### Changed

- **Unified binary**: Merged `post-cortex` and `post-cortex-daemon` into single `pcx` binary supporting both stdio and SSE transports
- **Simplified CLI**: `pcx` (stdio mode), `pcx start` (daemon mode), `pcx status`, `pcx stop`
- **Updated README**: Simplified documentation reflecting unified binary architecture

### Added

- **Docker MCP Registry support**: Added Dockerfile and configuration for Docker-based deployment
- **Auto-daemon startup**: Stdio mode automatically starts daemon in background if not running

## [0.1.5] - 2025-11-24

### Added

- **CLI workspace and session management**: New commands `post-cortex-daemon workspace` and `post-cortex-daemon session` for comprehensive workspace and session administration
- **Integration tests for admin tasks**: Comprehensive test suite covering workspace creation, session management, and administrative operations
- **Gemini API compatibility**: Flattened semantic search schema structure for full compatibility with Gemini's JSON Schema validation

### Changed

- **Semantic search API schema**: Replaced nested SearchScope struct with flattened scope_type and scope_id fields for better API compatibility
- **CLI daemon binary**: Enhanced with extensive workspace and session management capabilities

### Technical

- Added JsonSchema derives to CodeReference and ContextUpdateItem for better API documentation
- Improved schema descriptions for MCP tool documentation
- Enhanced daemon parameter parsing with safe error handling

## [0.1.4] - 2025-11-22

### Added

- **Unified semantic search with workspace scoping**: New `semantic_search` MCP tool replaces separate session/global tools with unified interface supporting session, workspace, and global scopes
- **Workspace persistence**: Full RocksDB persistence and hydration for workspaces enabling reliable cross-session workspace management
- **Entity graph relations in vectorizer**: Entities now include up to 5 related concepts in vector embeddings through actual graph traversal
- **Comprehensive integration tests**: Added unified search integration tests covering session, workspace, and global scopes with complex multi-session scenarios

### Fixed

- **Query cache cross-session result leakage**: Critical bug where semantic search returned cached results from wrong sessions due to missing params_hash validation in similarity-based cache lookups
- **Semantic search missing updates after hot context eviction**: Implemented vectorized_update_ids tracking to ensure all updates remain searchable even after eviction from hot/warm tiers to cold storage
- **Unsafe unwraps in daemon server**: Replaced panic-prone .unwrap() calls with safe error handling in parameter parsing logic to prevent server crashes

### Changed

- **Entity graph integration**: Replaced TODO placeholders with actual graph relationship traversal in content vectorizer for both sequential and Rayon parallel processing paths
- **Vectorization strategy**: Modified to iterate incremental_updates instead of hot+warm context only, with DashSet-based tracking for efficient re-vectorization skipping

### Improved

- **Code quality**: Resolved all clippy warnings (27 → 0) including redundant clones, ptr_arg issues, and simplified boolean logic
- **Error handling**: Replaced unsafe unwraps with safe .map().unwrap_or_else() patterns in daemon endpoints
- **Performance**: Incremental vectorization with O(1) DashSet lookups for already-vectorized updates

### Removed

- Unused `extract_text_from_compressed_update` method from codebase

## [0.1.3] - 2025-11-21

### Fixed

- Data persistence: Fixed launchd /tmp data loss on reboot by moving to ~/.post-cortex/data
- Daemon logs moved to ~/.post-cortex/logs with absolute paths
- Unified storage: stdio + daemon now use ~/.post-cortex/data
- Fixed query_conversation_context data field in response

## [0.1.2] - 2025-11-20

### Fixed

- Standardized on MultilingualMiniLM embedding model
- Fixed text extraction to include question titles
- Semantic search similarity improved from 11% to 94%+

## [0.1.1] - 2025-11-19

### Fixed

- Fixed launchd configuration for proper daemon startup
- Resource URL interpolation in Homebrew formula

## [0.1.0] - 2025-11-18

### Added

- Initial release of Post-Cortex
- Lock-free conversation memory system with zero-deadlock guarantees
- Three-tier memory hierarchy (Hot/Warm/Cold)
- Local AI-powered semantic search with HNSW indexing
- NER-powered knowledge graph extraction
- 24 MCP tools for conversation context management
- RocksDB persistence layer
- Daemon mode with HTTP/SSE transport
