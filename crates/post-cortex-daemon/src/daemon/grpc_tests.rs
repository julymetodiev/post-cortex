// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The `pcx.v1` gRPC service (`post_cortex_transport::memory`) over memory
//! as the daemon composes it.

use std::sync::Arc;

use post_cortex_memory::{Memory, MemoryModule};
use post_cortex_proto::pb;
use post_cortex_proto::pb::post_cortex_server::PostCortex;
use post_cortex_transport::memory::PcxGrpcService;
use tonic::Request;

use crate::compose::BUILD;
use pb::*;

/// The service under test and the memory it serves.
struct Served {
    grpc: PcxGrpcService,
    memory: Arc<Memory>,
}

impl std::ops::Deref for Served {
    type Target = PcxGrpcService;

    fn deref(&self) -> &PcxGrpcService {
        &self.grpc
    }
}

/// The gRPC service over `memory`, with `memory` at hand.
fn served(memory: Arc<Memory>) -> Served {
    Served {
        grpc: PcxGrpcService::new(
            Arc::clone(&memory),
            BUILD,
            MemoryModule::new(Arc::clone(&memory)).unwrap().kinds(),
        ),
        memory,
    }
}

mod capture;
mod lifecycle;
mod projects;
mod records;
mod service;
mod transfer;
mod views;
