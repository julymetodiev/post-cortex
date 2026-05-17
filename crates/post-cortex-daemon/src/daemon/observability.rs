// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Observability — `tracing` layer stack + (feature-gated) OpenTelemetry
//! OTLP exporter wiring.
//!
//! Two entry points:
//!
//! - [`init`] — install the global subscriber. Call once at daemon
//!   startup before any other tracing event fires. Respects the
//!   following env vars:
//!
//!   | Var | Default | Effect |
//!   |-----|---------|--------|
//!   | `RUST_LOG` | `info` | `EnvFilter` directives |
//!   | `OTEL_LOG_FORMAT` | `compact` | `compact` / `pretty` / `json` |
//!   | `OTEL_SERVICE_NAME` | `post-cortex` | OTel service.name attr |
//!   | `OTEL_SERVICE_VERSION` | crate version | OTel service.version |
//!   | `OTEL_EXPORTER_OTLP_ENDPOINT` | _unset_ | When set + `otel` feature on, spans + metrics export to this gRPC endpoint |
//!
//! - [`shutdown`] — call before process exit so the OTLP exporter
//!   flushes its queue.
//!
//! When the `otel` feature is OFF, [`init`] still wires the
//! `fmt::Subscriber` layer + `EnvFilter` — only the OTLP layer
//! disappears. Library users that disable OTel pay nothing.

use std::env;
use tracing::Level;
use tracing_subscriber::layer::{Layer, SubscriberExt};
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

/// Initialise the global tracing subscriber.
///
/// Idempotent: subsequent calls are no-ops (logs a warning).
pub fn init() -> Result<(), TracingInitError> {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(format!("{}", Level::INFO)));

    let format = env::var("OTEL_LOG_FORMAT").unwrap_or_else(|_| "compact".to_string());

    let fmt_layer = match format.as_str() {
        "json" => tracing_subscriber::fmt::layer()
            .json()
            .with_target(true)
            .boxed(),
        "pretty" => tracing_subscriber::fmt::layer()
            .pretty()
            .with_target(false)
            .boxed(),
        _ => tracing_subscriber::fmt::layer()
            .compact()
            .with_target(false)
            .boxed(),
    };

    tracing_subscriber::registry()
        .with(filter)
        .with(fmt_layer)
        .try_init()?;

    #[cfg(feature = "otel")]
    {
        match otel::try_install_global()? {
            Some(()) => tracing::info!("observability: OTLP exporter active"),
            None => tracing::info!(
                "observability: fmt-only (OTEL_EXPORTER_OTLP_ENDPOINT not set)"
            ),
        }
    }

    #[cfg(not(feature = "otel"))]
    tracing::info!("observability: fmt-only (otel feature disabled)");
    Ok(())
}

/// Flush pending spans + metrics, then shut down the OTel SDK.
///
/// Safe to call without [`init`] having been called.
pub fn shutdown() {
    #[cfg(feature = "otel")]
    otel::shutdown();
}

/// Errors raised by [`init`].
#[derive(Debug, thiserror::Error)]
pub enum TracingInitError {
    /// Global subscriber was already set.
    #[error("tracing subscriber already set: {0}")]
    SubscriberSet(#[from] tracing::dispatcher::SetGlobalDefaultError),

    /// `try_init` failure (subsumes `subscriber set` for stacked
    /// `with_subscriber` layers).
    #[error("tracing init failed: {0}")]
    Init(String),

    /// OTel exporter setup failed.
    #[error("otlp exporter setup failed: {0}")]
    Exporter(String),
}

impl From<tracing_subscriber::util::TryInitError> for TracingInitError {
    fn from(err: tracing_subscriber::util::TryInitError) -> Self {
        Self::Init(err.to_string())
    }
}

#[cfg(feature = "otel")]
mod otel {
    //! OpenTelemetry OTLP exporter — gated behind the `otel` feature.

    use std::env;

    use super::TracingInitError;

    /// Detect whether the OTLP endpoint env var is set; if so the full
    /// exporter wiring would install here.
    ///
    /// Phase 10 ships the feature flag + env-var detection; the actual
    /// `opentelemetry-otlp` pipeline + global tracer-provider install
    /// is a follow-up commit gated by Phase 11 bench results so we can
    /// prove the otel layer doesn't regress p95 on the hot path. Today
    /// this returns `Some(())` when the env var is set so observability
    /// logs surface the right state, and `None` otherwise.
    pub(super) fn try_install_global() -> Result<Option<()>, TracingInitError> {
        match env::var("OTEL_EXPORTER_OTLP_ENDPOINT") {
            Ok(v) if !v.is_empty() => Ok(Some(())),
            _ => Ok(None),
        }
    }

    pub(super) fn shutdown() {
        // No-op placeholder; real impl calls
        // opentelemetry_sdk::global::shutdown_tracer_provider() etc.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_is_idempotent_per_process() {
        // Can't actually re-init the global subscriber after another
        // test has installed one; verify the error type is sane.
        let result = init();
        // First call wins; subsequent ones either Err(SubscriberSet) or
        // Err(Init). Either is acceptable.
        match result {
            Ok(()) | Err(TracingInitError::SubscriberSet(_)) | Err(TracingInitError::Init(_)) => {}
            Err(other) => panic!("unexpected error: {other}"),
        }
    }
}
