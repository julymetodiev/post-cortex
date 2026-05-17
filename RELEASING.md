# Releasing post-cortex

This repo is a Cargo workspace of 8 publishable crates. Releases go out as one synchronised wave — every crate bumps to the same version, and `cargo workspaces publish` walks them in topological order so dependents always resolve published versions of their dependencies.

## Pre-flight

Before tagging:

1. `cargo fmt --all --check`
2. `cargo clippy --workspace --all-targets --all-features -- -D warnings`
3. `cargo test --workspace --lib --all-features`
4. `cargo test --workspace --doc --all-features`
5. `cargo doc --workspace --no-deps --all-features`
6. `cargo audit` and `cargo deny check`
7. `cargo workspaces publish --from-git --dry-run --yes`

CI runs all of the above on every push; the publish dry-run also fires on tag push. Don't tag if the dry-run failed.

## Cutting the release

1. Bump every crate's version. We pin `version = "0.X.Y"` in `[workspace.package]` and inherit via `version.workspace = true`, so a single edit at `Cargo.toml:[workspace.package].version` updates all members:

   ```sh
   # Edit Cargo.toml: [workspace.package] version = "0.X.Y"
   # Edit Cargo.toml: [workspace.dependencies] every `post-cortex-*` entry version = "0.X.Y"
   cargo check --workspace        # confirm Cargo.lock updates cleanly
   ```

2. Generate / update `CHANGELOG.md`:

   ```sh
   git cliff --tag v0.X.Y --output CHANGELOG.md
   ```

   (`git-cliff` config lives in `cliff.toml` once it exists; otherwise hand-write the section.)

3. Commit:

   ```sh
   git add Cargo.toml Cargo.lock CHANGELOG.md
   git commit -m "Release v0.X.Y"
   ```

4. Tag and push:

   ```sh
   git tag v0.X.Y
   git push origin main v0.X.Y
   ```

5. CI's `publish-dry-run` job runs against the tag. If green, run the real publish:

   ```sh
   cargo workspaces publish --from-git --yes
   ```

   `cargo-workspaces` walks the topological order: `post-cortex-proto` → `post-cortex-core` → `post-cortex-embeddings` → `post-cortex-storage` → `post-cortex-memory` → `post-cortex-mcp` → `post-cortex-daemon` → `post-cortex`. If any crate fails to publish, the rest abort and you can retry after fixing.

6. The `Release` workflow in `.github/workflows/release.yml` separately builds and publishes the `pcx` binary for macOS Intel / Apple Silicon / Linux x86_64.

## Hotfix protocol

For a security or correctness patch on a stable 0.X.Y line:

1. Branch from the latest 0.X.* tag (`git checkout -b release/0.X v0.X.Y`).
2. Cherry-pick / write the fix.
3. Bump every crate to `0.X.(Y+1)`.
4. Tag `v0.X.(Y+1)` and run the same publish flow.
5. Forward-port the fix to `main` if the bug also affects it.

## Yanking

`cargo yank --vers 0.X.Y -p post-cortex-*` for each affected crate. Update [SECURITY.md](SECURITY.md) supported-versions table and announce on the GitHub release thread.

## Crate-by-crate publish order (reference)

```
post-cortex-proto
post-cortex-core
post-cortex-embeddings
post-cortex-storage
post-cortex-memory
post-cortex-mcp
post-cortex-daemon
post-cortex
```

`cargo workspaces` computes this automatically — listed here only as a sanity check.
