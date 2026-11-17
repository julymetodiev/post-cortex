# The Linux part of the gate (scripts/cargo.sh linux): the toolchain and the
# system packages CI's clippy and cargo doc jobs install (.github/workflows/ci.yml),
# plus the well-known .proto files (Debian ships them apart, in libprotobuf-dev)
# and libclang for the bindgen of rocksdb.
FROM rust:1
RUN apt-get update \
 && apt-get install -y --no-install-recommends protobuf-compiler libprotobuf-dev libclang-dev \
 && rm -rf /var/lib/apt/lists/* \
 && rustup component add clippy
