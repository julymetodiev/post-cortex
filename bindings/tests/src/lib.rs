// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The protos of the bindings' end-to-end test (tests/bindings.rs): the
//! greet example's (`examples/greet`) and the relay fixture's
//! (`fixtures/relay_module.*`, whose `Helper` the test serves in the host's
//! process).

#![forbid(unsafe_code)]

/// The relay fixture's package.
pub mod relay {
    tonic::include_proto!("test.relay.v1");
}

/// The greet example's package.
pub mod greet {
    tonic::include_proto!("example.greet.v1");
}

/// The descriptors of the relay fixture's package, which the helper's
/// manifest carries.
pub const RELAY_DESCRIPTORS: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/relay_descriptor.bin"));
