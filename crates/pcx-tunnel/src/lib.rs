// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A daemon's tunnel to the cloud's gateway (`docs/connect.md`, section 2):
//! the daemon opens one `pcx.tunnel.v1.Tunnel/Serve` stream and the gateway
//! forwards HTTP requests over it, each an exchange of frames, their bytes
//! unchanged.
//!
//! An adapter of both sides: [`daemon::run`] serves a router over the
//! tunnel (the daemon's composition starts it), and [`gateway::serve`]
//! answers a daemon's call and gives the gateway a [`gateway::Link`] to
//! send requests over. Neither reads a body: what the requests mean is the
//! router's and the gateway's business.

mod endpoint;
mod error;
mod exchanges;
mod frame;
mod session;

pub mod daemon;
pub mod gateway;
pub mod pairing;

pub use error::TunnelError;
pub use exchanges::Incoming;

/// The body bytes of an exchange either side may send before the other
/// grants more: 256 KiB.
pub const WINDOW: u32 = 256 * 1024;

/// The exchanges open at once on one tunnel at most.
pub const MAX_EXCHANGES: u32 = 256;

/// The header the gateway sets on an exchange, saying what it is for
/// (docs/connect.md section 7): [`USE_RUNNER`], a hosted tenant's agent
/// calling a tool module the daemon lends; absent, a person of the space.
/// The gateway removes any a client sent.
pub const USE_HEADER: &str = "x-pcx-use";

/// [`USE_HEADER`]'s value for a runner's call.
pub const USE_RUNNER: &str = "runner";
