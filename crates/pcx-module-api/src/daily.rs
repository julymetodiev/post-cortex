// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A module's work once a day (the pruning of a log): first ten minutes
//! after its start, since the host makes memory callable once every module
//! has started, then every 24 hours, until the module stops.

use std::future::Future;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

/// How long after the start the work is first done.
const FIRST: Duration = Duration::from_secs(600);

/// How often it is done then.
const EVERY: Duration = Duration::from_secs(24 * 3_600);

/// Do `work` [`FIRST`] after now, then every [`EVERY`], until `stop` is
/// cancelled.
pub fn daily<F, W>(stop: CancellationToken, work: F)
where
    F: Fn() -> W + Send + 'static,
    W: Future<Output = ()> + Send,
{
    tokio::spawn(async move {
        let mut tick = tokio::time::interval_at(tokio::time::Instant::now() + FIRST, EVERY);
        loop {
            tokio::select! {
                () = stop.cancelled() => break,
                _ = tick.tick() => work().await,
            }
        }
    });
}
