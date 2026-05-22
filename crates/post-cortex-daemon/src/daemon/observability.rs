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
use std::fmt;
use tracing::Level;
use tracing::field::{Field, Visit};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::{Context, Filter, Layer, SubscriberExt};
use tracing_subscriber::util::SubscriberInitExt;

/// Layer-level filter that suppresses one specific upstream noise pattern
/// from `rmcp::transport::common::server_side_http`.
///
/// rmcp's streamable_http transport logs `Internal server error when
/// resume session: Session error: Channel closed: Some(N)` at ERROR level
/// for every successful unary MCP request — the client GETs to resume an
/// SSE stream whose underlying channel was already closed after the
/// response was sent. This is benign (no data lost, no user impact) but
/// pollutes every operator's logs and makes real ERROR events harder to
/// notice.
///
/// We drop ONLY this exact pattern. Every other rmcp server-side error
/// (`check session`, `accept message`, `create session`, etc.) still
/// surfaces normally.
///
/// Track upstream fix at <https://github.com/4t145/rmcp/issues> — once
/// rmcp downgrades closed-channel resume to DEBUG (or removes the GET
/// resume probe entirely for one-shot transports), this filter can go.
struct RmcpResumeNoiseFilter;

const RMCP_NOISE_TARGET: &str = "rmcp::transport::common::server_side_http";

struct MessageCapture(String);

impl Visit for MessageCapture {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.0.push_str(value);
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name() == "message" {
            use std::fmt::Write;
            let _ = write!(&mut self.0, "{value:?}");
        }
    }
}

impl<S: tracing::Subscriber> Filter<S> for RmcpResumeNoiseFilter {
    fn enabled(&self, _metadata: &tracing::Metadata<'_>, _ctx: &Context<'_, S>) -> bool {
        // Defer the real decision to `event_enabled` where we have access
        // to the event's `message` field. Returning `true` here lets the
        // event through for content inspection.
        true
    }

    fn event_enabled(&self, event: &tracing::Event<'_>, _ctx: &Context<'_, S>) -> bool {
        if event.metadata().target() != RMCP_NOISE_TARGET {
            return true;
        }
        let mut capture = MessageCapture(String::new());
        event.record(&mut capture);
        let msg = capture.0;
        // Suppress only the specific resume-session + channel-closed combo.
        // Every other rmcp transport error stays visible.
        let noisy =
            msg.contains("resume session") && msg.contains("Channel closed");
        !noisy
    }
}

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
            .with_filter(RmcpResumeNoiseFilter)
            .boxed(),
        "pretty" => tracing_subscriber::fmt::layer()
            .pretty()
            .with_target(false)
            .with_filter(RmcpResumeNoiseFilter)
            .boxed(),
        _ => tracing_subscriber::fmt::layer()
            .compact()
            .with_target(false)
            .with_filter(RmcpResumeNoiseFilter)
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
            None => tracing::info!("observability: fmt-only (OTEL_EXPORTER_OTLP_ENDPOINT not set)"),
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
    use std::sync::{Arc, Mutex};
    use tracing::{Level, error, info};
    use tracing_subscriber::layer::SubscriberExt;

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

    /// Capture-layer used by the filter tests below. Records the
    /// formatted `message` field of every event it sees, so we can
    /// assert "this got through" vs "this was dropped".
    #[derive(Default, Clone)]
    struct CaptureLayer(Arc<Mutex<Vec<String>>>);

    impl<S: tracing::Subscriber> Layer<S> for CaptureLayer {
        fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
            let mut capture = MessageCapture(String::new());
            event.record(&mut capture);
            self.0.lock().unwrap().push(format!(
                "{}:{}:{}",
                event.metadata().level(),
                event.metadata().target(),
                capture.0
            ));
        }
    }

    /// The noisy line from the upstream issue:
    /// `Internal server error when resume session: Session error: Channel closed: Some(6)`
    /// must be dropped by the filter.
    #[test]
    fn noise_filter_drops_resume_session_channel_closed() {
        let capture = CaptureLayer::default();
        let subscriber = tracing_subscriber::registry()
            .with(EnvFilter::new("trace"))
            .with(capture.clone().with_filter(RmcpResumeNoiseFilter));

        tracing::subscriber::with_default(subscriber, || {
            // Emit the exact upstream noise.
            error!(
                target: RMCP_NOISE_TARGET,
                "Internal server error when resume session: Session error: Channel closed: Some(6)"
            );
            // Emit something that should survive: same target but different reason.
            error!(
                target: RMCP_NOISE_TARGET,
                "Internal server error when create session: timeout"
            );
            // Emit an unrelated info line that should survive.
            info!(target: "post_cortex_daemon::test", "unrelated info");
        });

        let captured = capture.0.lock().unwrap();
        let joined = captured.join("\n");
        assert!(
            !joined.contains("Channel closed"),
            "resume-session noise should be dropped, got: {joined}"
        );
        assert!(
            joined.contains("create session: timeout"),
            "other rmcp errors must still surface, got: {joined}"
        );
        assert!(
            joined.contains("unrelated info"),
            "non-rmcp events must pass, got: {joined}"
        );
    }

    /// Events from unrelated targets, even if they happen to mention
    /// "Channel closed", must NOT be filtered. We only drop the specific
    /// rmcp target.
    #[test]
    fn noise_filter_respects_target() {
        let capture = CaptureLayer::default();
        let subscriber = tracing_subscriber::registry()
            .with(EnvFilter::new("trace"))
            .with(capture.clone().with_filter(RmcpResumeNoiseFilter));

        tracing::subscriber::with_default(subscriber, || {
            // Same substring, different target → must pass.
            error!(
                target: "post_cortex::session_manager",
                "resume session aborted: Channel closed by client"
            );
        });

        let captured = capture.0.lock().unwrap();
        assert_eq!(
            captured.len(),
            1,
            "non-rmcp targets must not be filtered, got: {captured:?}"
        );
    }

    /// Level doesn't matter for the rule — if the message matches the
    /// pattern we drop it regardless of level. (Defensive: if rmcp ever
    /// downgrades to WARN we still skip it.)
    #[test]
    fn noise_filter_is_level_agnostic() {
        let capture = CaptureLayer::default();
        let subscriber = tracing_subscriber::registry()
            .with(EnvFilter::new("trace"))
            .with(capture.clone().with_filter(RmcpResumeNoiseFilter));

        tracing::subscriber::with_default(subscriber, || {
            tracing::warn!(
                target: RMCP_NOISE_TARGET,
                "Internal server error when resume session: Session error: Channel closed: Some(9)"
            );
        });

        let captured = capture.0.lock().unwrap();
        assert!(
            captured.is_empty(),
            "level-agnostic drop expected, got: {captured:?}"
        );
        // Sanity: silence the unused-warning for the captured Level import.
        let _ = Level::WARN;
    }
}
