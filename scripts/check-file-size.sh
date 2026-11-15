#!/usr/bin/env bash
# Rule 6 of docs/ARCHITECTURE.md: no Rust source file over 500 lines.
#
# Checks every .rs file in the repository (tracked or untracked, minus
# .gitignore'd paths such as target/). Exempt:
#   - generated code (prost/tonic output carries an `@generated` marker);
#   - upstream code patched for the workspace (third_party/, each with its
#     PATCH.md), which is no member of the workspace;
#   - the files listed in scripts/file-size-allowlist.txt, which are still
#     over the limit and are split by a later migration wave.
#
# Fails when a non-exempt file is over the limit, and when an allowlisted
# file is missing or already within the limit (take it off the list), so
# the allowlist only ever shrinks.
#
# Usage: scripts/check-file-size.sh [--list]
#   --list  print every file over the limit (to regenerate the allowlist)
set -euo pipefail

LIMIT=500
root="$(cd "$(dirname "$0")/.." && pwd)"
allowlist="$root/scripts/file-size-allowlist.txt"
cd "$root"

over_limit() {
    git ls-files --cached --others --exclude-standard -- '*.rs' | sort -u |
        while IFS= read -r file; do
            [ -f "$file" ] || continue
            case "$file" in third_party/*) continue ;; esac
            if head -n 5 "$file" | grep -q '@generated'; then
                continue
            fi
            lines=$(wc -l <"$file" | tr -d ' ')
            if [ "$lines" -gt "$LIMIT" ]; then
                printf '%s %s\n' "$lines" "$file"
            fi
        done
}

if [ "${1:-}" = "--list" ]; then
    over_limit | sort -k2
    exit 0
fi

allowed=""
if [ -f "$allowlist" ]; then
    allowed="$(grep -v '^[[:space:]]*#' "$allowlist" | sed 's/[[:space:]]*$//' | grep -v '^$' || true)"
fi

status=0
current="$(over_limit)"

while IFS=' ' read -r lines file; do
    [ -n "${file:-}" ] || continue
    if ! grep -qxF "$file" <<<"$allowed"; then
        echo "error: $file has $lines lines (limit $LIMIT); split it by responsibility" >&2
        status=1
    fi
done <<<"$current"

while IFS= read -r file; do
    [ -n "$file" ] || continue
    if [ ! -f "$file" ]; then
        echo "error: $file is on the allowlist but does not exist; remove it from $allowlist" >&2
        status=1
    elif ! grep -q " $file\$" <<<"$current"; then
        echo "error: $file is within $LIMIT lines now; remove it from $allowlist" >&2
        status=1
    fi
done <<<"$allowed"

if [ "$status" -eq 0 ]; then
    count=$(grep -c . <<<"$allowed" || true)
    echo "file size check passed ($count allowlisted files still over $LIMIT lines)"
fi
exit "$status"
