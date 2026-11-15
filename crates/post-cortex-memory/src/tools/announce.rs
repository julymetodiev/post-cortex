// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Memory's events on the organisation's bus (docs/host.md, events of a
//! module): each record written becomes `post-cortex.memory.record_added`,
//! once per workspace its session belongs to. They come from memory's
//! change feed, which never blocks a writer: when the feed falls behind,
//! the records it lost are not announced (logged).

use std::sync::Arc;

use pbjson_types::Any;
use pcx_module_api::{MemoryWrite, ModuleErrorKind, ModuleHost, Scope};
use post_cortex_proto::pb::RecordAdded;
use prost::Message;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::Memory;

/// The event's name.
pub const RECORD_ADDED: &str = "post-cortex.memory.record_added";

/// The event's message.
pub const RECORD_ADDED_TYPE: &str = "pcx.v1.RecordAdded";

/// The scope an announcement is made in: memory's own, in the record's
/// workspace.
fn scope(workspace_id: &str) -> Scope {
    Scope {
        tenant: String::new(),
        profile: String::new(),
        session_id: None,
        workspace_id: (!workspace_id.is_empty()).then(|| workspace_id.to_string()),
        author: None,
        caller_module: None,
        call_depth: 0,
        turn: None,
        reach: None,
        role: None,
        read_only: false,
        person: None,
        deadline: std::time::Instant::now() + pcx_module_api::DEFAULT_DEADLINE,
        cancel: CancellationToken::new(),
    }
}

/// Announce the records `memory` writes through `host`'s bus until `stop`.
pub async fn announce_records(
    host: Arc<dyn ModuleHost>,
    memory: Arc<Memory>,
    stop: CancellationToken,
) {
    let bus = match host.bus().await {
        Ok(bus) => bus,
        Err(e) => {
            debug!("memory announces no records: {e}");
            return;
        }
    };
    let mut writes = memory.changes.memory_writes();
    loop {
        let written = tokio::select! {
            () = stop.cancelled() => return,
            written = writes.next() => written,
        };
        let Some(written) = written else { return };
        for write in written {
            let MemoryWrite::Record {
                workspace_id,
                id,
                record_type: Some(record_type),
                deleted: false,
            } = write.write
            else {
                continue;
            };
            let payload = Any {
                type_url: format!("type.googleapis.com/{RECORD_ADDED_TYPE}"),
                value: RecordAdded {
                    record: id,
                    workspace_id: workspace_id.clone(),
                    record_type,
                }
                .encode_to_vec()
                .into(),
            };
            match bus
                .publish(&scope(&workspace_id), RECORD_ADDED, payload)
                .await
            {
                Ok(_) => {}
                Err(e) if e.kind == ModuleErrorKind::FailedPrecondition => {
                    debug!("the host keeps no events: {e}");
                    return;
                }
                Err(e) => warn!("a record was not announced: {e}"),
            }
        }
    }
}
