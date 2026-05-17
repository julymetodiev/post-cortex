# Performance & quality baseline — post-cortex 0.1.23

Captured before the multi-crate workspace refactor (branch `refactor/mcp-split-tools`, commit `0a9c507`). These numbers are the regression gates for the planned Phases 1–14 (`/Users/julius/.claude/plans/stateful-hugging-hopper.md`). Any phase that regresses p95 by >10% on any of the canonical operations below blocks further phases until investigated.

## Environment

- Host: macOS Darwin 25.4.0 / Apple Silicon
- Toolchain: `rustc 1.95.0 (2026-04-14)` / `cargo 1.95.0`
- Features baseline: `--all-features` (i.e. `embeddings,surrealdb-storage`)
- Storage backend: RocksDB unless noted

## Build

- `cargo build --all-features` (cold): **5m 24s** (workspace currently builds 766 crate deps via surrealdb + reqwest)
- `cargo build --all-features` (incremental, one-file edit): not measured here; will be re-baselined after Phase 1 workspace split.

## Test matrix

| Selector | Pass | Fail | Note |
|---|---|---|---|
| `cargo test --lib --features embeddings` | **148** | **1** | Pre-existing failure: `daemon::validate::tests::test_validate_session_action_invalid` (TODO.md notes this as the baseline failure across recent splits). Treat as "OK-broken" until Phase 9 error typing revisits MCP validation. |

Integration tests under `tests/` were not run for the Phase 0 baseline (they require a running daemon and serialise on RocksDB/SurrealDB). They will be measured at Phase 7 boundary.

## Clippy state (no strict suite yet)

`cargo clippy --all-targets --all-features` against today's implicit clippy defaults:

- **103 warnings** total across lib + tests (default level).
- **1 hard error** in `tests/property_vector_db.rs`: `clippy::approx_constant` — a literal `3.14…` should be `f64::consts::PI`. Fix as part of Phase 9 (`unwrap_used`/`expect_used` lint enable).

Once the strict lint suite from the plan ships (`clippy::pedantic` + `nursery` + `unwrap_used` + `expect_used` + `forbid(unsafe_code)`), warning count will spike before settling. Track delta phase-by-phase.

## Benchmarks (criterion)

- `benches/query_cache_bench.rs` defines `bench_cache_search` (load = 16/64/256) and `bench_cache_insert` (100 inserts), wired with `criterion_group! + criterion_main!`.
- **Cannot produce measurements today**: the bench is missing the corresponding `[[bench]]` block in `Cargo.toml` with `harness = false`, so `cargo bench` runs it under the default libtest harness which yields `0 measured`.
- **Action for Phase 1**: add
  ```toml
  [[bench]]
  name = "query_cache_bench"
  harness = false
  ```
  to `crates/post-cortex-core/Cargo.toml`, then re-run and append numbers below.

### Canonical hot-path operations to benchmark (Phase 11 deliverable)

The plan calls out four operations as the regression gates. Each must have a criterion bench with p50/p95/p99 numbers recorded here against a 10k-update test corpus:

| Operation | Path | Today p95 | Target p95 |
|---|---|---|---|
| `update_context` | `core::memory_system::system::update_conversation_context` | _TBD_ | single-digit ms (TODO.md:142 non-blocking write goal) |
| `semantic_search` | `core::semantic_query_engine::SemanticQueryEngine::search` | _TBD_ | <100ms at 10k docs |
| `query_context` | `tools::mcp::query::query_conversation_context` | _TBD_ | <50ms (keyword search, no embed) |
| `assemble_context` | `core::context_assembly::assemble_context` | _TBD_ | <200ms |

Numbers will be filled in at Phase 0 close-out once the benches land in Phase 11; for now the baseline file documents the intent.

## Public API surface

`cargo public-api --simplified` snapshot deferred to Phase 13 (CI gate). The current crate's surface is described by `src/lib.rs` re-exports plus the eight `pub mod` declarations (audit log in `/Users/julius/.claude/plans/stateful-hugging-hopper.md`, section "Public surface today").

Approximate surface size (today, single crate): re-exports = 4 typed items + 8 modules (`core, daemon, graph, session, storage, summary, tools, workspace`). After Phase 8 the public surface should match this from `post_cortex::*` via the facade.

## Notes for downstream phases

- Phase 9 (error typing) will land the strict lint suite that turns the 103 implicit warnings into hard CI failures; this is expected and the baseline file should not be re-touched until then.
- Phase 11 (optimizations) opens with the missing `[[bench]]` Cargo entry and a scripted latency harness (probably `examples/perf_harness.rs`) populating the four-row table above.
- The single pre-existing test failure stays unfixed across the refactor unless we are deliberately touching MCP validation in Phase 6/9.
