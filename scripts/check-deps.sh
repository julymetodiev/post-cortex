#!/usr/bin/env bash
# Every dependency at its latest release (CLAUDE.md, LAW: every dependency at its
# latest version; owner 2026-11-17). scripts/check_deps.py says what it checks.
#
#   scripts/check-deps.sh            this repository
#   scripts/check-deps.sh ../post-cortex-cloud
set -euo pipefail
exec python3 -I "$(dirname "$0")/check_deps.py" "$@"
