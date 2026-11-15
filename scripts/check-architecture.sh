#!/usr/bin/env bash
# Enforces the rules of docs/ARCHITECTURE.md section 2 that a grep can see.
# Each check counts violations in non-test code; a count above its budget in
# scripts/architecture-budget.txt fails, and a count below it fails too until
# the budget is lowered (the budget only goes down, to 0).
#   --list   print the current counts in budget format
set -euo pipefail
cd "$(dirname "$0")/.."

# Every file the checks read is tracked; a missing one fails here, so a
# check never counts nothing for a file it could not read.
for input in scripts/architecture-budget.txt scripts/approved-crate-items.txt \
  scripts/grpc-json-edges.txt scripts/proto-json-fields.awk; do
  [ -f "$input" ] || { echo "check-architecture: $input is missing" >&2; exit 1; }
done

# Test-only files: test modules and dirs, test helpers (`test_*.rs`), the
# conformance suites, benches, examples and build scripts.
TEST_FILES='(^|/)tests?(/|\.rs$)|_tests?(/|\.rs$)|(^|/)test_[a-z_]+\.rs$|/conformance(/|\.rs$)|/benches/|/examples/|(^|/)build\.rs$'
src_files() { # $@ = path prefixes; prints non-test .rs files
  git ls-files --cached --others --exclude-standard -- "$@" \
    | grep -E '\.rs$' \
    | grep -v -E "$TEST_FILES" || true
}
count() { # $1 = regex, rest = files on stdin
  local re=$1 n=0 f
  while read -r f; do
    [ -f "$f" ] || continue
    # drop the test module: everything from the first `#[cfg(test)]` (or
    # `#[cfg(all(test, ...))]`) that opens an inline module (`mod x {`); a
    # `#[cfg(test)] mod x;` declaration (a file of its own) and the item
    # under any other test attribute are dropped alone, the rest counts.
    n=$(( n + $(awk '/^[[:space:]]*#\[cfg\((all\()?test[,)]/ { t = 1; next }
                     t && /mod [A-Za-z0-9_]+ *\{/ { exit }
                     t { t = 0; next }
                     { print }' "$f" | grep -c -E "$re" || true) ))
  done
  echo "$n"
}

declare -A NOW
# Rule 7: module paths with an API
NOW[pub_super]=$(src_files crates bindings | count 'pub\(super\)')
NOW[super_paths]=$(src_files crates bindings | count '(^|[^a-z_])super::' )
# Rule 1: the domain is pure (no clock, no runtime state, no wire schema)
NOW[core_clock]=$(src_files crates/post-cortex-core | count 'Utc::now\(|SystemTime::now\(|Instant::now\(')
NOW[core_runtime]=$(src_files crates/post-cortex-core | count 'dashmap|DashMap|DashSet|tokio::|arc_swap|ArcSwap')
NOW[core_proto]=$(src_files crates/post-cortex-core | count 'post_cortex_proto')
# T7: no process-env reads below the composition root
NOW[env_below_root]=$(src_files crates/post-cortex-core crates/post-cortex-storage crates/post-cortex-embeddings crates/post-cortex-memory crates/post-cortex-transport | count 'env::var(_os)?\(')
# Rule 4 / section 3: capabilities use ports; the daemon picks the adapters
NOW[memory_adapters]=$(src_files crates/post-cortex-memory | count 'RealRocksDBStorage|SurrealDBStorage|rocksdb_storage::|surrealdb_storage::')
# Rule 4: a capability service reaches only the ports of its row, given at
# composition; it never reaches into the runtime's ports.
NOW[capability_ports]=$(src_files crates/post-cortex-memory/src/capabilities | count 'rt\.(stores|index\(\)|engine\(\)|archive\(\)|reranker\(\)|accurate_reranker\(\))')

# Rule 2 / the LAW: no logic in transports. A transport or tool handler maps
# its wire onto a capability or service call; it never reaches the host, the
# filesystem, a process or a storage adapter itself.
NOW[transport_reach]=$(src_files crates/post-cortex-transport/src crates/post-cortex-memory/src/tools/handlers 'crates/pcx-*/src/handlers*' | count 'ModuleHost|\.manifests\(|std::fs|tokio::fs|Command::new|cap_std|rocksdb::|surrealdb::|globset|ignore::')

# Rule 10 (W23, everything gRPC): a service's data is typed. A JSON-shaped
# proto field counts unless it is a JSON edge (scripts/grpc-json-edges.txt);
# a module's gRPC handlers never touch JSON.
json_edges() { grep -v -E '^[[:space:]]*(#|$)' scripts/grpc-json-edges.txt; }
NOW[grpc_json_fields]=$(for f in crates/post-cortex-proto/proto/*.proto; do awk -f scripts/proto-json-fields.awk "$f"; done \
  | grep -c -v -x -F -f <(json_edges) || true)
NOW[grpc_json_handlers]=$(src_files 'crates/pcx-*/src/handlers*' | count 'serde_json')

# docs/modules/dynamic.md: a module in another language works only through
# the bindings to the Rust core; no Python or TS file outside `bindings/`
# carries generated `pcx.module.v1` code or speaks that package itself.
NOW[foreign_modules]=$(git ls-files --cached --others --exclude-standard \
  | grep -E '\.(py|pyi|ts|tsx|mts|cts|js|mjs|cjs)$' | grep -v -E '^bindings/' \
  | while read -r f; do [ -f "$f" ] && grep -c -E 'pcx_module_pb|pcx_module_grpc|pcx\.module\.v1|pcx/module/v1' "$f" || true; done \
  | awk '{ n += $1 } END { print n + 0 }')

# docs/modules/dynamic.md: a module's process is launched only through the port
# ModuleLauncher; the board is asked about code by one implementation
# (pcx-approval); no script names Python's version (bindings/python/.python-version).
NOW[module_launch]=$(src_files crates/pcx-host/src/process.rs crates/pcx-host/src/process | grep -v -F crates/pcx-host/src/process/direct.rs | count 'Command::new')
NOW[approval_asks]=$(src_files crates | grep -v -E '^crates/(pcx-approval|pcx-board|pcx-telegram)/' | count 'Risk::Code')
NOW[python_version_copies]=$(git ls-files --cached --others --exclude-standard -- scripts | while read -r f; do [ -f "$f" ] && grep -E -c '3\.1[0-9]\.[0-9]' "$f" || true; done | awk '{ n += $1 } END { print n + 0 }')

# A build script that compiles protos from outside its package tells cargo to
# run again on their change (cargo watches only the package's own files; found
# 2026-11-14: greet carried the module API of 2026-11-01). Each such script
# without a `rerun-if-changed` counts.
NOW[stale_protos]=$(git ls-files --cached --others --exclude-standard | grep -E '(^|/)build\.rs$' \
  | while read -r f; do
      [ -f "$f" ] && grep -q -E '"\.\./[^"]*proto' "$f" && ! grep -q 'rerun-if-changed' "$f" && echo "$f"
    done | grep -c . || true)

# Rule 5 / the LAW: no patches (non-test code; each count may only fall, to 0)
NOW[allow_attrs]=$(src_files crates bindings | count '#!?\[allow\(')
NOW[unwrap_expect]=$(src_files crates bindings | count '\.unwrap\(\)|\.expect\(')
NOW[swallowed]=$(src_files crates bindings | count 'let _ = |\.ok\(\);')
NOW[debug_names]=$(src_files crates bindings | count 'format!\("\{:\?\}"')
NOW[string_errors]=$(src_files crates bindings | count 'to_string\(\)\.contains\(|err_str')
NOW[anyhow_libs]=$(src_files crates/post-cortex-core crates/post-cortex-ports crates/post-cortex-storage crates/post-cortex-embeddings crates/post-cortex-memory crates/post-cortex-transport | count 'anyhow::')
# Rule 7: the other ways to open a module upward
NOW[pub_in_path]=$(src_files crates bindings | count 'pub\(in ')
NOW[path_attrs]=$(src_files crates bindings | count '#\[path')

# Section 3: dependency direction between crates (normal [dependencies] only)
deps() { # $1 = crate dir; prints its normal dependency names
  awk '/^\[dependencies\]/{on=1;next} /^\[/{on=0} on && /^[a-z0-9_-]+ *[.=]/{sub(/ *[.=].*/,"");print}' "crates/$1/Cargo.toml"
}
NOW[core_deps]=$(deps post-cortex-core | grep -c -E '^(post-cortex-proto|post-cortex-storage|post-cortex-embeddings|post-cortex-memory|tokio|dashmap|arc-swap)$' || true)
NOW[memory_adapter_deps]=$(deps post-cortex-memory | grep -c -E '^(post-cortex-storage|post-cortex-embeddings|rocksdb|surrealdb)$' || true)
# The client never opens storage: it talks to a daemon or the cloud over
# their public APIs only (W7).
NOW[cli_deps]=$(deps post-cortex-cli | grep -c -E '^(post-cortex-memory|post-cortex-storage|post-cortex-embeddings|post-cortex-ports|post-cortex-transport|post-cortex-daemon|pcx-host|rocksdb|surrealdb)$' || true)

# --new BASE: no added line in non-test code (diff against BASE, plus untracked files) may
# break the LAW in CLAUDE.md / ARCHITECTURE.md section 2, even when the totals fall. Run by
# every agent before it reports, by the coordinator before a commit, and in CI.
if [ "${1:-}" = "--new" ]; then
  base=${2:-HEAD}
  re='pub\(super\)|pub\(crate\)|pub\(in |#\[path|(^|[^a-z_*])super::|#!?\[allow\(|\.unwrap\(\)|\.expect\(|let _ = |\.ok\(\);|format!\("\{:\?\}"|to_string\(\)\.contains\(|err_str|anyhow::'
  prod() { grep -v -E "$TEST_FILES"; }
  # added lines of production code: a line counts only if it sits before the file's first
  # `#[cfg(test)]` (any indentation); untracked files are added as a whole.
  added() { # $1 = file; prints "file: line" for added lines before its test module
    local f=$1 cut
    # The first `#[cfg(test)]` that opens an inline module (`mod x {`); a
    # `#[cfg(test)] mod x;` declaration does not start test code.
    cut=$(awk '/^[[:space:]]*#\[cfg\((all\()?test/ { t = NR; next }
               t && NR == t + 1 && /mod [A-Za-z0-9_]+ *\{/ { print t; exit }
               { t = 0 }' "$f")
    cut=${cut:-999999}
    # An untracked file is added as a whole: its diff against nothing.
    if git ls-files --error-unmatch "$f" >/dev/null 2>&1; then
      changes() { git diff -U0 "$base" -- "$f"; }
    else
      changes() { git diff --no-index -U0 /dev/null "$f" || true; }
    fi
    # A `pub(crate)` item that was `pub(crate)` before (its line changed,
    # e.g. new parameters) is not widened: only new ones count, unless the
    # owner approved the item (scripts/approved-crate-items.txt).
    approved=$(awk -v f="$f" '$1 == f { print $2 }' "$(dirname "$0")/approved-crate-items.txt" | tr '\n' ' ')
    changes | awk -v cut="$cut" -v f="$f" -v approved="$approved" '
      function item(line,   m) {
        if (match(line, /pub\(crate\) +(async +)?(fn|struct|enum|type|const|static|mod|trait) +[A-Za-z0-9_]+/)) {
          m = substr(line, RSTART, RLENGTH); sub(/.* /, "", m); return m
        }
        if (match(line, /pub\(crate\) +[a-z0-9_]+ *:/)) {
          m = substr(line, RSTART, RLENGTH); sub(/pub\(crate\) +/, "", m); sub(/ *:$/, "", m); return m
        }
        return ""
      }
      BEGIN { split(approved, ok, " "); for (i in ok) was[ok[i]] = 1 }
      { lines[NR] = $0 }
      /^-/ && !/^---/ { k = item($0); if (k != "") was[k] = 1 }
      END {
        for (i = 1; i <= NR; i++) {
          l = lines[i]
          if (l ~ /^@@/) { split(l, h, " "); split(h[3], a, ","); n = substr(a[1], 2) + 0; continue }
          if (l ~ /^\+\+\+/) continue
          if (l ~ /^\+/) {
            k = item(l)
            if (n < cut && !(k != "" && k in was)) print f ": " substr(l, 2)
            n++
          }
        }
      }'
  }
  hits=$( { for f in $( { git diff --name-only "$base" -- 'crates/*.rs' 'bindings/*.rs'; git ls-files --others --exclude-standard -- 'crates/*.rs' 'bindings/*.rs'; } | sort -u | prod); do
              [ -f "$f" ] && added "$f"
            done
          } | grep -E "$re" | grep -v -E ': \s*use super::\*;' | grep -v -E ': \s*//' || true)
  if [ -n "$hits" ]; then
    echo "error: new violations of the LAW (CLAUDE.md, ARCHITECTURE.md section 2) against $base:"
    echo "$hits" | head -60
    exit 1
  fi
  echo "no new violations against $base"
  exit 0
fi

if [ "${1:-}" = "--list" ]; then
  for k in "${!NOW[@]}"; do echo "$k ${NOW[$k]}"; done | sort
  exit 0
fi

fail=0
while read -r key budget; do
  [ -z "$key" ] && continue
  now=${NOW[$key]:-}
  [ -z "$now" ] && { echo "error: unknown check $key in the budget"; fail=1; continue; }
  if [ "$now" -gt "$budget" ]; then
    echo "error: $key has $now violations, budget $budget (docs/ARCHITECTURE.md section 2)"; fail=1
  elif [ "$now" -lt "$budget" ]; then
    echo "error: $key is down to $now; lower its budget in scripts/architecture-budget.txt to $now"; fail=1
  fi
done < scripts/architecture-budget.txt
[ $fail = 0 ] && echo "architecture check passed"
exit $fail
