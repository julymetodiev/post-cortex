#!/usr/bin/env bash
# The one way to run cargo here (owner, 2026-11-02). Cargo unifies features over
# the packages selected, so another `-p` set or feature set gives surrealdb and
# its tree a new hash and compiles them again. Every command below selects the
# same set, CI's. The hook .claude/hooks/pcx-cargo-guard.py refuses any other.
#
#   scripts/cargo.sh clippy           the workspace, all targets and features
#   scripts/cargo.sh test [FILTER]    nextest over the workspace
#   scripts/cargo.sh ignored FILTER   the #[ignore] tests matching FILTER
#   scripts/cargo.sh build            what test runs, without running it
#   scripts/cargo.sh bins             every binary (each module's process), debug,
#                                     for a local stack: built with every target, so
#                                     its features are the tests' (`--bins` alone
#                                     leaves the dev-dependencies out: a new surrealdb)
#   scripts/cargo.sh doc              doc tests (nextest skips them)
#   scripts/cargo.sh node             release build of pcx-module-node
#   scripts/cargo.sh pcx [TARGET]     release build of the pcx binary, as release.yml
#                                     builds it (target/release/pcx; with TARGET,
#                                     e.g. x86_64-apple-darwin, target/TARGET/release/pcx:
#                                     another target compiles everything once more)
#   scripts/cargo.sh rustdoc          the documentation of the workspace, as CI builds it
#                                     (a broken link inside it is an error)
#   scripts/cargo.sh deny             cargo deny check, as CI runs it (advisories,
#                                     licences, bans, sources; deny.toml); compiles nothing
#   scripts/cargo.sh cloud            clippy of ../post-cortex-cloud
#   scripts/cargo.sh cloud-test [FILTER]  nextest over ../post-cortex-cloud
#   scripts/cargo.sh cloud-deny       the advisories of ../post-cortex-cloud's Cargo.lock,
#                                     by this repository's deny.toml
#   scripts/cargo.sh cloud-bins       every binary of ../post-cortex-cloud (pcx-cloud),
#                                     debug, for a local stack: built with its tests, as
#                                     `bins`, so its features are cloud-test's
#
# Each run's whole log stays in target/cargo-logs. It ends by saying what it
# compiled, and loudly when that was surrealdb.
set -uo pipefail
cd "$(dirname "$0")/.."
root=$PWD
logs=$root/target/cargo-logs
mkdir -p "$logs"
what=${1:?usage: scripts/cargo.sh clippy|build|bins|test|ignored|doc|rustdoc|deny|node|pcx|cloud|cloud-test|cloud-deny|cloud-bins [FILTER]}
shift
log=$logs/$(date +%Y%m%d-%H%M%S)-$what.log
F=(--workspace --all-features)
MAX_VARIANTS=6
MAX_INCREMENTAL=350
case $what in
  clippy) cargo clippy "${F[@]}" --all-targets -- -D warnings >"$log" 2>&1; rc=$? ;;
  test) cargo nextest run "${F[@]}" --no-fail-fast "$@" >"$log" 2>&1; rc=$? ;;
  ignored) cargo nextest run "${F[@]}" --run-ignored only --no-capture "$@" >"$log" 2>&1; rc=$? ;;
  build) cargo nextest run "${F[@]}" --no-run >"$log" 2>&1; rc=$? ;;
  bins) cargo build "${F[@]}" --bins --tests >"$log" 2>&1; rc=$? ;;
  doc) cargo test "${F[@]}" --doc >"$log" 2>&1; rc=$? ;;
  node) cargo build --release -p pcx-module-node >"$log" 2>&1; rc=$? ;;
  # A released pcx carries its own RocksDB and Snappy, built from the crate's
  # source with its own headers: a machine's cargo configuration that links a
  # system library (ROCKSDB_LIB_DIR) would make a binary that runs only there.
  pcx) ROCKSDB_COMPILE=1 SNAPPY_COMPILE=1 ROCKSDB_INCLUDE_DIR=rocksdb/include \
    cargo build --release -p post-cortex-daemon --bin pcx --features surrealdb-storage,rerank-voyage ${1:+--target "$1"} >"$log" 2>&1; rc=$? ;;
  rustdoc) RUSTDOCFLAGS="-D warnings" cargo doc "${F[@]}" --no-deps --keep-going >"$log" 2>&1; rc=$? ;;
  deny) cargo deny check >"$log" 2>&1; rc=$? ;;
  cloud-deny) (cd ../post-cortex-cloud && cargo deny check --config "$root/deny.toml" advisories) >"$log" 2>&1; rc=$? ;;
  cloud) (cd ../post-cortex-cloud && cargo clippy --workspace --all-targets -- -D warnings) >"$log" 2>&1; rc=$? ;;
  cloud-test) (cd ../post-cortex-cloud && cargo nextest run --workspace --no-fail-fast "$@") >"$log" 2>&1; rc=$? ;;
  cloud-bins) (cd ../post-cortex-cloud && cargo build --workspace --bins --tests) >"$log" 2>&1; rc=$? ;;
  *) echo "scripts/cargo.sh: no command $what" >&2; exit 64 ;;
esac
compiled=$(grep -cE '^ *(Compiling|Checking) ' "$log")
heavy=$(grep -cE '^ *(Compiling|Checking) (surrealdb|librocksdb)' "$log")
case $what in
  # cargo deny warns of every duplicate version; what it refuses is an error.
  *deny) sed $'s/\x1b\\[[0-9;]*m//g' "$log" | grep -E '^error' -A7 | cut -c1-140 | head -60 ;;
  *) grep -E '^(error|warning)' -A7 "$log" | cut -c1-140 | head -40 ;;
esac
grep -E '^ *Summary|^test result|FAILED|FAIL |^advisories ' "$log" | cut -c1-140 | tail -12
echo "$what rc=$rc; compiled $compiled crates; log $log"
[ "$heavy" = 0 ] || echo "!!! surrealdb or rocksdb compiled ($heavy): say why to the owner"
# One set of features is four directories of surrealdb-core: its build script compiled and run,
# one check, one build. Mimir's rust-analyzer shares target/ with default features and adds two:
# its build script compiled and run (owner, 2026-11-04).
target=$root/target
case $what in cloud*) target=$root/../post-cortex-cloud/target ;; esac
variants=$(find "$target/debug/.fingerprint" -maxdepth 1 -name 'surrealdb-core-*' 2>/dev/null | wc -l | tr -d ' ')
if [ "$variants" -gt "$MAX_VARIANTS" ]; then
  echo "!!! $variants fingerprints of surrealdb-core, at most $MAX_VARIANTS: another set compiles it, or a change of Cargo.lock left the old ones"
  [ "$rc" != 0 ] || rc=70
fi
# Cargo never removes an incremental cache whose hash is no longer used (5652 directories, 164 GB):
# each change of a crate's dependencies leaves one. Removing them costs only the next compile of
# each workspace crate; another set compiling surrealdb is what the check above says.
incremental=$(find "$target/debug/incremental" -mindepth 1 -maxdepth 1 -type d 2>/dev/null | wc -l | tr -d ' ')
if [ "$incremental" -gt "$MAX_INCREMENTAL" ]; then
  rm -rf "$target/debug/incremental"
  echo "$incremental incremental caches, at most $MAX_INCREMENTAL: removed"
fi
exit "$rc"
