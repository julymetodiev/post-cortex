// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The smallest out-of-process module, as a third party writes one (W23):
//! its own package, `example.greet.v1`, whose `Greeter` service greets with
//! the greeting its configuration sets (`{"greeting": "Hello"}`); `Greet`
//! is a tool for agents and people, `ListGreetings` and `Forget` are for
//! its views only. Two views (ARCHITECTURE section 7, Module UI):
//! `greetings`, a tree over those RPCs, and `card`, an HTML view that
//! greets through the MCP Apps bridge.
//!
//! The host starts it with `PCX_MODULE_SOCKET` naming the socket to serve
//! on; give the host its id `greet`, the path of the built binary and its
//! SHA-256:
//!
//! ```sh
//! cargo build --release -p pcx-greet
//! sha256sum target/release/pcx-greet
//! ```

mod module;
mod service;
mod view;

pub use module::Greet;

/// The module's package: its messages, and the client of its service.
pub mod pb {
    tonic::include_proto!("example.greet.v1");
}

/// The descriptors of its package (with the module API's it imports),
/// which its manifest carries.
pub const DESCRIPTORS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/greet_descriptor.bin"));

/// The module's id; the host's configuration names the same.
pub const ID: &str = "greet";
