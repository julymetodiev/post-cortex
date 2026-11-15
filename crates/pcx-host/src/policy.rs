// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! One module under the host's policy: every call runs within a deadline,
//! stops on cancellation, has its panic caught and waits for a slot of the
//! module's concurrency limit (T4).

use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

use futures::FutureExt;
use parking_lot::Mutex;
use pcx_module_api::{
    LOG_LINES, LogLine, LogRing, LogSource, Module, ModuleError, ModuleErrorKind,
};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use tracing::Instrument;

use crate::breaker::{Breaker, Verdict};
use crate::stats::{Ended, ModuleHealth, Stats};

/// Calls one module runs at once; more wait for a slot within their
/// deadline.
const CONCURRENCY: usize = 16;

/// A module's call, ready to run.
pub type Call<T> = Pin<Box<dyn Future<Output = Result<T, ModuleError>> + Send>>;

/// A module with its policy state.
pub struct Guarded {
    module: Arc<dyn Module>,
    id: String,
    slots: Semaphore,
    stats: Stats,
    breaker: Breaker,
    /// What the host saw of it: its failed calls and its breaker opening.
    log: LogRing,
    /// What stops it from doing its work, as it reported last; empty:
    /// nothing.
    problem: Mutex<String>,
    /// Why its newest settings were not applied, as the host found when it
    /// reloaded (it runs with the ones before); empty: they are.
    unapplied: Mutex<String>,
}

impl Guarded {
    /// `module` under the host's policy.
    pub fn new(module: Arc<dyn Module>) -> Self {
        let id = module.manifest().id.clone();
        Self {
            module,
            id,
            slots: Semaphore::new(CONCURRENCY),
            stats: Stats::default(),
            breaker: Breaker::default(),
            log: LogRing::default(),
            problem: Mutex::default(),
            unapplied: Mutex::default(),
        }
    }

    /// The module's id.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The module.
    pub fn module(&self) -> &Arc<dyn Module> {
        &self.module
    }

    /// The module's health.
    pub fn health(&self) -> ModuleHealth {
        let (unapplied, problem) = (self.unapplied.lock().clone(), self.problem.lock().clone());
        ModuleHealth {
            // What a person fixes first: settings that were not taken.
            problem: match (unapplied.is_empty(), problem.is_empty()) {
                (true, _) => problem,
                (false, true) => unapplied,
                (false, false) => format!("{unapplied}. {problem}"),
            },
            ..self
                .stats
                .health(&self.id, self.breaker.open_for().is_some())
        }
    }

    /// The module reports `problem`, what stops it from doing its work
    /// (empty: nothing does): kept for its health, and written in its log
    /// when it is not what it reported last.
    pub fn report(&self, problem: &str) {
        let mut kept = self.problem.lock();
        if *kept == problem {
            return;
        }
        if problem.is_empty() {
            self.log.push(LogSource::Host, "it works again".to_string());
            tracing::info!(module = %self.id, "the module works again");
        } else {
            self.log
                .push(LogSource::Host, format!("it cannot work: {problem}"));
            tracing::warn!(module = %self.id, problem, "the module cannot work");
        }
        *kept = problem.to_string();
    }

    /// Its log: what the host saw of it and what its process wrote, the
    /// newest [`LOG_LINES`], the oldest first.
    pub fn log(&self) -> Vec<LogLine> {
        let mut lines = self.log.lines();
        lines.extend(self.module.output());
        lines.sort_by_key(|line| line.at);
        let skip = lines.len().saturating_sub(LOG_LINES);
        lines.split_off(skip)
    }

    /// The host did not apply the module's new settings because of `why`,
    /// and it runs with the ones before (empty: its settings are applied):
    /// kept for its health beside what it reports itself, and written in
    /// its log when it changes.
    pub fn unapplied(&self, why: &str) {
        let mut kept = self.unapplied.lock();
        if *kept == why {
            return;
        }
        if why.is_empty() {
            self.log
                .push(LogSource::Host, "its settings are applied".to_string());
        } else {
            self.log.push(LogSource::Host, why.to_string());
            tracing::warn!(module = %self.id, why, "the module's new settings were not applied");
        }
        *kept = why.to_string();
    }

    /// Run `call` (built from the module) until `deadline` or `cancel`,
    /// with its panic caught; counted in the module's health.
    pub async fn run<T>(
        &self,
        cancel: &CancellationToken,
        deadline: Instant,
        operation: &'static str,
        call: impl FnOnce(Arc<dyn Module>) -> Call<T>,
    ) -> Result<T, ModuleError> {
        self.run_judged(cancel, deadline, operation, call, |_| None)
            .await
    }

    /// [`Guarded::run`] for a call whose answer may itself tell a failure
    /// (a forwarded gRPC answer's status): `judge` names it, and the answer
    /// is returned as it came, counted as that failure.
    pub async fn run_judged<T>(
        &self,
        cancel: &CancellationToken,
        deadline: Instant,
        operation: &'static str,
        call: impl FnOnce(Arc<dyn Module>) -> Call<T>,
        judge: impl FnOnce(&T) -> Option<ModuleError>,
    ) -> Result<T, ModuleError> {
        let started = Instant::now();
        if let Some(left) = self.breaker.open_for() {
            self.stats.record(started.elapsed(), Ended::Refused);
            return Err(ModuleError::new(
                ModuleErrorKind::Unavailable,
                format!(
                    "module {} keeps failing; retried in {}s",
                    self.id,
                    left.as_secs().max(1)
                ),
            ));
        }
        let budget = deadline.saturating_duration_since(started);
        let span = tracing::info_span!("module", module = %self.id, operation);
        let work = async {
            // The semaphore is never closed.
            let Ok(_slot) = self.slots.acquire().await else {
                return (Ended::Failed, Err(unavailable()));
            };
            match AssertUnwindSafe(call(Arc::clone(&self.module)))
                .catch_unwind()
                .await
            {
                Ok(Ok(value)) => (Ended::Done, Ok(value)),
                Ok(Err(e)) => (Ended::Failed, Err(e)),
                Err(_) => (
                    Ended::Panicked,
                    Err(ModuleError::new(
                        ModuleErrorKind::Internal,
                        format!("module {} panicked", self.id),
                    )),
                ),
            }
        }
        .instrument(span.clone());
        let (ended, result) = tokio::select! {
            ended = tokio::time::timeout(budget, work) => ended.unwrap_or_else(|_| (
                Ended::TimedOut,
                Err(ModuleError::new(
                    ModuleErrorKind::DeadlineExceeded,
                    format!("module {} ran past its deadline ({budget:?})", self.id),
                )),
            )),
            () = cancel.cancelled() => (
                Ended::Cancelled,
                Err(ModuleError::new(ModuleErrorKind::DeadlineExceeded, "the caller cancelled")),
            ),
        };
        let elapsed = started.elapsed();
        // An answer that tells a failure counts as one.
        let told = result.as_ref().ok().and_then(judge);
        let ended = match (&told, ended) {
            (Some(_), Ended::Done) => Ended::Failed,
            (_, ended) => ended,
        };
        self.stats.record(elapsed, ended);
        let failure = match (&result, &told) {
            (Err(e), _) | (Ok(_), Some(e)) => Some(e),
            (Ok(_), None) => None,
        };
        let verdict = match (failure, ended) {
            (_, Ended::Cancelled) => Verdict::Unknown,
            (None, _) => Verdict::Answered,
            (Some(e), _) => Verdict::of_error(e.kind),
        };
        if let (Some(e), Verdict::Failed) = (failure, verdict) {
            self.log
                .push(LogSource::Host, format!("{operation} failed: {e}"));
        }
        if let Some(open) = self.breaker.record(verdict, &self.id) {
            self.log.push(
                LogSource::Host,
                format!("it keeps failing: its calls are refused for {open:?}"),
            );
        }
        // Only the module's own failure warns: a refused input or a caller
        // who gave up is the caller's business.
        span.in_scope(|| match (failure, verdict) {
            (None, _) => tracing::debug!(?elapsed, "module call done"),
            (Some(e), Verdict::Failed) => {
                tracing::warn!(?elapsed, error = %e, "module call failed")
            }
            (Some(e), Verdict::Answered | Verdict::Unknown) => {
                tracing::debug!(?elapsed, error = %e, "module call refused")
            }
        });
        result
    }
}

/// The error of a call that found the module's slots closed.
fn unavailable() -> ModuleError {
    ModuleError::new(ModuleErrorKind::Unavailable, "the module is stopping")
}
