# Public API baseline — post-cortex 0.1.23

This directory holds the `cargo public-api` snapshots taken just before the multi-crate workspace refactor, so the post-Phase-8 facade re-exports can be diffed against the original surface to prove that nothing the user could reach today disappeared without intent.

## Capture process

`cargo public-api --simplified` was started during Phase 0 but had not finished its rustdoc build at commit time (the all-features rustdoc on this crate takes several minutes). The snapshot will be generated as the first step of **Phase 13 (CI matrix rewrite)** and committed as `surface.txt` next to this README, alongside per-crate snapshots once the workspace exists:

- `surface-post-cortex-0.1.23.txt` — single-crate baseline
- `surface-post-cortex-core-0.2.0.txt`, `surface-post-cortex-mcp-0.2.0.txt`, `surface-post-cortex-daemon-0.2.0.txt`, `surface-post-cortex-proto-0.2.0.txt`, `surface-post-cortex-0.2.0.txt` — after the split

CI runs `cargo public-api --diff <baseline> <current>` and fails on **breaking changes**, allowing the workflow to gate every PR on backwards-compatibility through the facade.

## Today's surface (manual list, from `src/lib.rs`)

```text
pub mod core
pub mod daemon
pub mod graph
pub mod session
pub mod storage
pub mod summary
pub mod tools
pub mod workspace
pub use core::error::Result
pub use core::error::SystemError
pub use core::memory_system::ConversationMemorySystem
pub use core::memory_system::SystemConfig
pub use summary::StructuredSummaryView
pub use summary::SummaryGenerator
```

Plus everything reachable through those eight `pub mod`s. After Phase 8, the facade crate must reproduce the same top-level identifiers under `post_cortex::*` or expose a documented rename.
