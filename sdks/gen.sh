#!/usr/bin/env bash
# Regenerate the gRPC code of the SDKs from crates/post-cortex-proto/proto/pcx.proto.
# The generated files are committed; CI runs this and fails on a diff.
#
# Needs: uv, node + npm. Usage: sdks/gen.sh [python|typescript]
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
proto_dir="$here/../crates/post-cortex-proto/proto"
what="${1:-all}"
# pcx.proto and the files it imports (each SDK needs their messages too); never
# the module protocol (pcx.module.v1, docs/modules/dynamic.md `foreign_modules`).
protos=(pcx.proto views.proto pcx_blocks.proto)

if [[ "$what" == all || "$what" == python ]]; then
  out="$here/python/src/post_cortex/_pb"
  # Generated anew, so a file of a proto no longer imported does not stay.
  rm -rf "$out" && mkdir -p "$out"
  # mypy-protobuf writes the type stubs (messages and the gRPC stub typed), which
  # `mypy --strict` checks the client against.
  py=(uv run --quiet --no-project --python 3.12 --with "grpcio-tools==1.85.1" --with "mypy-protobuf==5.1.0" python -m grpc_tools.protoc -I "$proto_dir")
  (cd "$here/python" && "${py[@]}" --python_out="$out" --mypy_out="$out" "${protos[@]}" && \
    "${py[@]}" --grpc_python_out="$out" --mypy_grpc_out="$out" pcx.proto)
  # grpc_tools and mypy-protobuf write `import pcx_pb2`; the package needs relative imports.
  for f in "$out"/*.py "$out"/*.pyi; do
    sed -i.bak -E 's/^import ([a-z_]+_pb2) as /from . import \1 as /' "$f"
    rm -f "$f.bak"
  done
  touch "$out/__init__.py"
fi

if [[ "$what" == all || "$what" == typescript ]]; then
  ts="$here/typescript"
  # The protoc bundled with the pinned grpcio-tools, as for Python, so the output
  # (whose header names the protoc version) is the same on every machine.
  (cd "$ts" && npm ci --silent && rm -rf src/gen && mkdir -p src/gen && \
    uv run --quiet --no-project --python 3.12 --with "grpcio-tools==1.85.1" python -m grpc_tools.protoc \
      --plugin=protoc-gen-ts_proto=./node_modules/.bin/protoc-gen-ts_proto \
      --ts_proto_out=src/gen \
      --ts_proto_opt=outputServices=grpc-js,esModuleInterop=true,env=node,useOptionals=messages,forceLong=number,oneof=unions,importSuffix=.js \
      -I "$proto_dir" "$proto_dir/pcx.proto")
fi
