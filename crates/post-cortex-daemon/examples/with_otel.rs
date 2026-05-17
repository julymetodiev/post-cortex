// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Example: start the post-cortex daemon with OpenTelemetry export.
//!
//! Run with the `otel` feature enabled and the OTLP endpoint pointed at
//! a local collector:
//!
//! ```sh
//! # In one terminal — local OTel collector (gRPC, port 4317):
//! docker run --rm -p 4317:4317 -p 16686:16686 \
//!     -e COLLECTOR_OTLP_ENABLED=true \
//!     jaegertracing/all-in-one:latest
//!
//! # In another terminal — daemon with OTel:
//! OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4317 \
//!     OTEL_SERVICE_NAME=post-cortex-dev \
//!     OTEL_LOG_FORMAT=json \
//!     RUST_LOG=info \
//!     cargo run --features otel --example with_otel -p post-cortex-daemon
//! ```
//!
//! Then visit http://localhost:16686 to see the traces.

use post_cortex_daemon::daemon::observability;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    observability::init()?;

    tracing::info!("post-cortex demo daemon starting");

    // Fire a few demo spans so the collector has something to render.
    for i in 0..5 {
        demo_request(i).await;
    }

    tracing::info!("done; flushing OTel exporter");
    observability::shutdown();
    Ok(())
}

#[tracing::instrument(fields(request_id = id))]
async fn demo_request(id: u32) {
    tracing::info!("handling request");
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    inner_work().await;
}

#[tracing::instrument]
async fn inner_work() {
    tracing::debug!("doing inner work");
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
}
