// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The memory-write feed (ARCHITECTURE section 7, modules together): each
//! change memory's ChangeLog holds goes to the modules that follow it, in
//! order, fail-open; a feed that fell behind says so once.

use std::sync::Arc;
use std::time::{Duration, Instant};

use pcx_host::Host;
use pcx_module_api::{MemoryWrite, Scope};
use post_cortex_memory::Memory;
use post_cortex_memory::capabilities::Written;
use tokio_util::sync::CancellationToken;
use tracing::warn;
use uuid::Uuid;

/// How long the modules that follow one change have to answer it (each
/// subscription's own deadline applies first).
const FEED_DEADLINE: Duration = Duration::from_secs(30);

/// Feed `memory`'s writes to `host`'s modules until `stop` or the memory
/// instance goes away.
pub async fn feed_memory_writes(host: Arc<Host>, memory: Arc<Memory>, stop: CancellationToken) {
    let mut writes = memory.changes.memory_writes();
    loop {
        let next = tokio::select! {
            () = stop.cancelled() => return,
            next = writes.next() => next,
        };
        let Some(writes) = next else {
            return;
        };
        for Written { write, reach } in writes {
            let scope = Scope {
                tenant: String::new(),
                profile: String::new(),
                session_id: None,
                workspace_id: workspace_of(&write),
                author: None,
                caller_module: None,
                call_depth: 0,
                turn: None,
                // The modules that follow the write act within the reach of
                // the call that made it (docs/connect.md 4a).
                reach: reach.map(|ids| ids.iter().map(Uuid::to_string).collect()),
                role: None,
                read_only: false,
                person: None,
                deadline: Instant::now() + FEED_DEADLINE,
                cancel: stop.child_token(),
            };
            let dispatched = host.dispatch_memory_write(&scope, &write).await;
            for failure in dispatched.failures {
                warn!(
                    module = failure.module,
                    error = %failure.error,
                    "a module failed a memory write (fail-open)"
                );
            }
        }
    }
}

/// The workspace a write names, if any.
fn workspace_of(write: &MemoryWrite) -> Option<String> {
    match write {
        MemoryWrite::Document { workspace_id, .. } | MemoryWrite::Record { workspace_id, .. } => {
            (!workspace_id.is_empty()).then(|| workspace_id.clone())
        }
        MemoryWrite::FellBehind => None,
    }
}
