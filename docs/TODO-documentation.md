# Documentation TODO — follow-up sweep

## Context

Post-Cortex 0.2.0 ships with crate-level rustdoc + per-crate `README.md` + doc comments on every public **type**, **trait**, and **public method**. What's **not** yet documented are the **struct fields** and **enum variants** inside many of those types — `RUSTDOCFLAGS="-D missing_docs" cargo doc --workspace --no-deps --all-features` reports **1073 undocumented items** at the time of the 0.2.0 cut.

`missing_docs` is set to `warn` (not `deny`) in `workspace.lints.rust` so `docs.rs` builds succeed; the gap is surfaced as warnings in CI. This document tracks the cleanup so a 0.3.0 release can flip the lint to `deny`.

## Current state (2026-05-17, commit pre-0.2.0)

`cargo doc --workspace --no-deps --all-features` is **clean** (no errors, no `-D` failures). `cargo clippy --workspace --all-targets --all-features` is **clean** (0 warnings, 0 errors).

`RUSTDOCFLAGS="-D missing_docs" cargo doc --workspace --no-deps --all-features` errors at **1073** items, distributed:

| Crate | Missing items |
|-------|---------------|
| `post-cortex-core` | 431 |
| `post-cortex-memory` | 337 |
| `post-cortex-mcp` | 153 |
| `post-cortex-storage` | 80 |
| `post-cortex-daemon` | 67 |
| **Total** | **1068** (the ~5 delta vs 1073 is intra-doc-link warnings that don't count as `missing_docs`) |

Crates with **zero** missing docs: `post-cortex-proto` (entire crate is `tonic::include_proto!` generated, opted out of the lint), `post-cortex-embeddings` (cleaned up during Phase 12 — the 10 items flagged earlier were fixed), and the `post-cortex` facade (re-exports inherit the source doc).

## What's already covered

- Crate-level rustdoc on every `lib.rs` (intro paragraph, module map, status note, MSRV).
- `README.md` per crate (elevator pitch, install snippet, runnable example, feature matrix, license).
- Every public **type** has at least a one-line `///` doc.
- Every public **trait** has a doc comment.
- Every public **fn** at the crate root has at least a synopsis.
- The 8-crate workspace `README.md` at the repo root has the layout table + dependency graph.
- `SECURITY.md` + `RELEASING.md` at the workspace root.

## What's missing

Per-category breakdown (sampled from the rustdoc error stream):

1. **Struct fields** — the majority. Many domain types (`ContextUpdate`, `StoredEntity`, `SystemHealth`, etc.) have field docs but several public records (e.g. some `pub`-fields in `memory_system::*` config structs) do not.
2. **Enum variants** — `SystemError`, `pipeline::PipelineError`, `services::SessionAction` / `WorkspaceAction` / `EntityAction` / `AdminAction` variants need per-variant docs. The variant *body* fields also need docs (e.g. `Backpressure { queue }` — `queue` field undocumented).
3. **Submodule declarations** — `pub mod` inside `crates/post-cortex-mcp/src/lib.rs` for the 7 tool modules (session, update_context, query, search, analysis, workspace, schemas) lack module-level doc paragraphs. Similar for some `core::*` sub-modules.
4. **Generated re-exports** — `pub use foo::*;` lines at crate roots inherit the source doc; no action needed for these.

## Priority order for the cleanup sweep

When this sweep happens (target: 0.3.0):

1. **Crate roots first** (~30 items) — `pub mod` declarations that are missing top-of-module paragraphs. Highest visibility per character written.
2. **Public enum variants** (~150 items) — every variant in `SystemError`, `memory::Error`, `mcp::Error`, `daemon::Error`, `pipeline::PipelineError`, and the `services::*Action` enums.
3. **Public struct fields on canonical types** (~200 items) — `ContextUpdate`, `EntityData`, `EntityRelationship`, `UpdateContent`, `CodeReference`, `StoredEntity`, `SessionCheckpoint`, `StoredWorkspace`, `VectorMetadata` (already done), `SearchMatch`, `VectorDbStatsSnapshot` (already done in Phase 13).
4. **Public struct fields on config types** (~300 items) — `SystemConfig`, `EmbeddingConfig`, `VectorDbConfig`, `QueryCacheConfig`, `ContentVectorizerConfig`, `DaemonConfig`, `StorageConfig`. Each field needs a one-line `/// description`.
5. **Everything else** (~400 items) — internal-ish public types that survived the visibility audit.

Estimate: 4-6 hours of focused writing.

## How to run the audit

```sh
RUSTDOCFLAGS="-D missing_docs" cargo doc --workspace --no-deps --all-features 2>&1 | grep -c "^error"
# 1073 today

# Per-crate count:
RUSTDOCFLAGS="-D missing_docs" cargo doc --workspace --no-deps --all-features 2>&1 \
    | grep -A 1 "^error: missing" \
    | grep -oE 'crates/[^/]+' \
    | sort | uniq -c | sort -rn

# Find specific files with the most missing items:
RUSTDOCFLAGS="-D missing_docs" cargo doc --workspace --no-deps --all-features 2>&1 \
    | grep -A 1 "^error: missing" \
    | grep -E "  --> crates/" \
    | awk '{print $2}' \
    | awk -F: '{print $1}' \
    | sort | uniq -c | sort -rn | head -20
```

## Flipping the lint

Once 0 items remain:

1. Edit `Cargo.toml` at workspace root: change `[workspace.lints.rust] missing_docs = "warn"` → `missing_docs = "deny"`.
2. Run `cargo doc --workspace --no-deps --all-features` — should still pass.
3. CI's `docs` job already uses `RUSTDOCFLAGS="-D rustdoc::broken_intra_doc_links"` — adding `-D missing_docs` is a one-line change in `.github/workflows/ci.yml`.
4. Bump the workspace to 0.3.0 (the docs-complete release).

## Related follow-ups

- **Doctest sweep** — every public function should have at least one runnable doctest. Currently only the headline functions (`ConversationMemorySystem::new`, `LocalEmbeddingEngine::new`, etc.) have doctests. Track separately.
- **`examples/`** — `post-cortex-core/examples/quickstart.rs`, `post-cortex-core/examples/custom_storage.rs`, `post-cortex-mcp/examples/embed_in_custom_server.rs`, `post-cortex-proto/examples/grpc_client.rs` — referenced in the plan but only `with_otel.rs` shipped in 0.2.0.
- **Module-level `mod.rs` rustdoc** — most submodules have the boilerplate `//! Persisted record types ...` line; expand to full module-level rustdoc when touching for the field-docs sweep.
