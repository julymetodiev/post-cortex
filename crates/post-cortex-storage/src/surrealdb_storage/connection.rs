// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A connection to SurrealDB kept answering (the prod outage of
//! 2026-11-08). The SDK pings the server every 5 s but never checks that an
//! answer comes, and reconnects only when the socket reports an error, so a
//! connection whose packets are lost without a reset (a firewall that drops
//! them) hangs every call forever (`tests/surreal_reset.rs`). A
//! [`Connection`] asks the server a trivial query every [`PROBE`]; when it
//! is not answered within [`ANSWER`] [`MISSES`] times in a row, it opens a
//! new connection the way it opened the first (sign-in, namespace and
//! database) and calls take that one from then on.

use std::sync::{Arc, Weak};
use std::time::Duration;

use arc_swap::ArcSwap;
use futures::future::BoxFuture;
use surrealdb::Surreal;
use surrealdb::engine::any::Any;
use tokio::task::JoinHandle;

/// How often the server is asked whether the connection answers.
pub const PROBE: Duration = Duration::from_secs(5);

/// How long a probe, or the opening of a new connection, may take.
pub const ANSWER: Duration = Duration::from_secs(5);

/// Probes unanswered in a row that replace the connection.
pub const MISSES: u32 = 2;

/// Opens a connection: connected, signed in, its namespace and database
/// selected.
pub type Opener =
    Arc<dyn Fn() -> BoxFuture<'static, surrealdb::Result<Surreal<Any>>> + Send + Sync>;

/// A connection to SurrealDB that is replaced when it stops answering.
pub struct Connection {
    current: ArcSwap<Surreal<Any>>,
    open: Opener,
    /// What it is a connection to, for the log.
    name: String,
}

impl Connection {
    /// The connection `open` opens, named `name` in the log, watched until
    /// it is dropped. Call within a Tokio runtime.
    ///
    /// # Errors
    /// The first connection does not open.
    pub async fn open(name: String, open: Opener) -> surrealdb::Result<Arc<Self>> {
        let db = open().await?;
        let connection = Arc::new(Self {
            current: ArcSwap::from_pointee(db),
            open,
            name,
        });
        tokio::spawn(watch(Arc::downgrade(&connection)));
        Ok(connection)
    }

    /// The connection calls use now.
    #[must_use]
    pub fn db(&self) -> Arc<Surreal<Any>> {
        self.current.load_full()
    }

    /// Asks the connection a trivial query, in a task of its own that waits
    /// for the answer however late it comes: the SDK logs an ERROR for an
    /// answer whose asker is gone (staging 2026-11-08, when a replaced
    /// connection's packets came back), so a probe is never given up.
    fn ask(&self) -> JoinHandle<bool> {
        let db = self.db();
        tokio::spawn(async move { db.query("RETURN 1;").await.is_ok() })
    }

    /// A new connection in place of the current one.
    async fn reopen(&self) -> Result<(), String> {
        match tokio::time::timeout(ANSWER, (self.open)()).await {
            Ok(Ok(db)) => {
                self.current.store(Arc::new(db));
                Ok(())
            }
            Ok(Err(e)) => Err(e.to_string()),
            Err(_) => Err(format!("it did not open within {ANSWER:?}")),
        }
    }
}

/// Probe `connection` every [`PROBE`] until it is dropped, and replace it
/// after [`MISSES`] probes in a row not answered within [`ANSWER`]. A probe
/// not answered in time is waited on again at the next one instead of a new
/// question; one left behind by a replacement keeps waiting in its task.
async fn watch(connection: Weak<Connection>) {
    let mut missed = 0;
    let mut unanswered: Option<JoinHandle<bool>> = None;
    loop {
        tokio::time::sleep(PROBE).await;
        let Some(connection) = connection.upgrade() else {
            return;
        };
        let mut probe = unanswered.take().unwrap_or_else(|| connection.ask());
        match tokio::time::timeout(ANSWER, &mut probe).await {
            Ok(Ok(true)) => {
                missed = 0;
                continue;
            }
            Ok(_) => {}
            Err(_) => unanswered = Some(probe),
        }
        missed += 1;
        if missed < MISSES {
            continue;
        }
        match connection.reopen().await {
            Ok(()) => {
                tracing::warn!(
                    database = %connection.name,
                    "SurrealDB did not answer {MISSES} probes in a row: a new connection replaced it"
                );
                missed = 0;
                unanswered = None;
            }
            Err(e) => tracing::error!(
                database = %connection.name,
                error = %e,
                "SurrealDB does not answer and a new connection did not open; trying again"
            ),
        }
    }
}
