#!/usr/bin/env bash
# The bindings end to end (docs/modules/dynamic.md): builds the Python
# extension (free-threaded CPython, abi3t) and the Node-API addon, then runs
# bindings/tests against the real host with Python, Deno and Node.
# PCX_PYTHON names the free-threaded CPython (default: bindings/python/.python-version).
set -euo pipefail
cd "$(dirname "$0")/.."

python_version=${PCX_PYTHON:-$(cat bindings/python/.python-version)}
venv=bindings/python/.venv
if [ ! -x "$venv/bin/python" ]; then
  uv venv --python "$python_version" "$venv"
fi
uv pip install --python "$venv/bin/python" --quiet "protobuf>=6"
(cd bindings/python && VIRTUAL_ENV="$PWD/.venv" maturin develop --release --quiet)

scripts/cargo.sh node
case "$(uname -s)" in
  Darwin) lib=target/release/libpcx_module_node.dylib ;;
  *) lib=target/release/libpcx_module_node.so ;;
esac
cp "$lib" "bindings/ts/pcx_module.$(node -p 'process.platform + "-" + process.arch').node"

# The message code is the checked-in one (scripts/bindings-gen.sh writes it).
(cd bindings/ts && npm install --silent)
(cd examples/greet/ts && npm install --silent)
(cd bindings/tests/fixtures && npm install --silent)

PCX_BINDINGS_PYTHON="$PWD/$venv/bin/python" \
PCX_BINDINGS_DENO="$(command -v deno)" \
PCX_BINDINGS_NODE="$(command -v node)" \
  scripts/cargo.sh ignored -E 'package(pcx-bindings-tests)'
