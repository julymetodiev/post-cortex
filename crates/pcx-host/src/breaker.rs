// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A module's breaker (T4): after [`FAILURES`] failures of the module in a
//! row (past its deadline, a panic, unavailable or internal), its calls and
//! events are refused at once for [`OPEN_FOR`]; then they reach it again,
//! and one more failure opens it again. A caller's mistake (an invalid
//! argument, an unknown name, a denied capability) is not the module
//! failing.

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use pcx_module_api::ModuleErrorKind;

/// Module failures in a row that open the breaker.
const FAILURES: u32 = 5;
/// How long an open breaker refuses calls.
pub const OPEN_FOR: Duration = Duration::from_secs(30);

/// One module's breaker.
#[derive(Debug, Default)]
pub struct Breaker {
    failures: AtomicU32,
    opened: Mutex<Option<Instant>>,
}

/// What a finished call says about the module.
#[derive(Debug, Clone, Copy)]
pub enum Verdict {
    /// It answered (with a value or the caller's mistake).
    Answered,
    /// It failed.
    Failed,
    /// Nothing: the caller gave up.
    Unknown,
}

impl Verdict {
    /// The verdict of an error of `kind`.
    pub fn of_error(kind: ModuleErrorKind) -> Self {
        match kind {
            ModuleErrorKind::Unavailable | ModuleErrorKind::Internal => Self::Failed,
            ModuleErrorKind::DeadlineExceeded => Self::Failed,
            ModuleErrorKind::InvalidArgument
            | ModuleErrorKind::NotFound
            | ModuleErrorKind::Denied
            | ModuleErrorKind::Conflict
            | ModuleErrorKind::FailedPrecondition
            | ModuleErrorKind::ResourceExhausted
            // The place it runs is not connected: not the module failing.
            | ModuleErrorKind::Offline => Self::Answered,
        }
    }
}

impl Breaker {
    /// How long the breaker stays open, if it is.
    pub fn open_for(&self) -> Option<Duration> {
        let opened = (*self.opened.lock())?;
        OPEN_FOR.checked_sub(opened.elapsed())
    }

    /// Count a finished call; how long the breaker opens for when this
    /// call opens it.
    pub fn record(&self, verdict: Verdict, module: &str) -> Option<Duration> {
        match verdict {
            Verdict::Answered => self.failures.store(0, Ordering::Relaxed),
            Verdict::Failed => {
                let failures = self.failures.fetch_add(1, Ordering::Relaxed) + 1;
                if failures >= FAILURES {
                    *self.opened.lock() = Some(Instant::now());
                    tracing::warn!(
                        module,
                        failures,
                        "module failing; breaker open for {OPEN_FOR:?}"
                    );
                    return Some(OPEN_FOR);
                }
            }
            Verdict::Unknown => {}
        }
        None
    }
}
