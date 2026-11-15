#!/usr/bin/env bash
# The message code of the bindings' example and test protos
# (docs/modules/dynamic.md): Python with protoc, TypeScript with
# protoc-gen-es. Never the module protocol's (pcx.module.v1).
set -euo pipefail
cd "$(dirname "$0")/.."

(cd bindings/ts && npm install --silent)
es=bindings/ts/node_modules/.bin/protoc-gen-es

gen() { # <proto root> <file> <python out> <ts out>
  protoc -I "$1" --python_out="$3" "$2"
  mkdir -p "$4"
  protoc -I "$1" --plugin=protoc-gen-es="$es" --es_out="$4" \
    --es_opt=target=ts,import_extension=ts "$2"
}

gen examples/greet/proto greet/v1/greet.proto examples/greet/python examples/greet/ts/gen
gen bindings/tests/proto relay/v1/relay.proto bindings/tests/fixtures bindings/tests/fixtures/gen
