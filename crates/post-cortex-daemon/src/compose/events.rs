// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The host's events in memory (docs/host.md, events of a module): the
//! host's [`EventStore`] port over memory's Events capability, so the
//! tenant's one writer keeps them, and the daily drop of the old ones.
//! The daemon and every hosted tenant compose the same.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use pbjson_types::{Any, Timestamp};
use pcx_host::{EventStore, StoredEvents};
use pcx_module_api::bus::payload_type;
use pcx_module_api::{ModuleError, ModuleErrorKind};
use post_cortex_core::core::event::{BusEvent as Kept, NewBusEvent};
use post_cortex_memory::Memory;
use post_cortex_memory::capabilities::{EventError, Events};
use post_cortex_proto::module::BusEvent;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};
use uuid::Uuid;

/// How often old events are dropped.
const DROP_INTERVAL: Duration = Duration::from_secs(24 * 3600);

/// The prefix of a payload's type URL.
const TYPE_URL: &str = "type.googleapis.com/";

/// The host's events, kept by memory.
pub struct MemoryEvents {
    events: Events,
}

impl MemoryEvents {
    /// The store over `memory`'s Events.
    #[must_use]
    pub fn new(memory: &Memory) -> Self {
        Self {
            events: memory.events.clone(),
        }
    }
}

fn invalid(message: String) -> ModuleError {
    ModuleError::new(ModuleErrorKind::InvalidArgument, message)
}

fn failed(e: EventError) -> ModuleError {
    let kind = match e {
        EventError::InvalidArgument(_) => ModuleErrorKind::InvalidArgument,
        EventError::Storage(_) => ModuleErrorKind::Internal,
    };
    ModuleError::new(kind, e.to_string())
}

/// The event the host stamped, as memory keeps it.
fn kept(event: BusEvent) -> Result<NewBusEvent, ModuleError> {
    let id =
        Uuid::parse_str(&event.id).map_err(|e| invalid(format!("event id {}: {e}", event.id)))?;
    let workspace_id = (!event.workspace_id.is_empty())
        .then(|| {
            Uuid::parse_str(&event.workspace_id)
                .map_err(|e| invalid(format!("workspace {}: {e}", event.workspace_id)))
        })
        .transpose()?;
    let at = event
        .at
        .ok_or_else(|| invalid(format!("event {} has no time", event.id)))?;
    let out_of_range = || invalid(format!("event {} has a time out of range", event.id));
    let nanos = u32::try_from(at.nanos).map_err(|_| out_of_range())?;
    let at = DateTime::<Utc>::from_timestamp(at.seconds, nanos).ok_or_else(out_of_range)?;
    let payload = event
        .payload
        .ok_or_else(|| invalid(format!("event {} has no payload", event.id)))?;
    let payload_type = payload_type(&payload)
        .map(str::to_string)
        .ok_or_else(|| invalid(format!("event {}: its payload names no type", event.id)))?;
    Ok(NewBusEvent {
        id,
        name: event.name,
        module: event.module,
        workspace_id,
        author: (!event.author.is_empty()).then_some(event.author),
        at,
        payload_type,
        payload: payload.value.to_vec(),
    })
}

/// An event memory keeps, on the wire.
fn wire(event: Kept) -> Result<BusEvent, ModuleError> {
    let e = event.event;
    Ok(BusEvent {
        id: e.id.to_string(),
        position: event.position,
        name: e.name,
        module: e.module,
        workspace_id: e.workspace_id.map(|w| w.to_string()).unwrap_or_default(),
        author: e.author.unwrap_or_default(),
        at: Some(Timestamp {
            seconds: e.at.timestamp(),
            nanos: i32::try_from(e.at.timestamp_subsec_nanos()).map_err(|_| {
                ModuleError::new(ModuleErrorKind::Internal, "a time names no nanosecond")
            })?,
        }),
        payload: Some(Any {
            type_url: format!("{TYPE_URL}{}", e.payload_type),
            value: e.payload.into(),
        }),
    })
}

#[async_trait]
impl EventStore for MemoryEvents {
    async fn append(&self, event: BusEvent) -> Result<BusEvent, ModuleError> {
        wire(self.events.append(kept(event)?).await.map_err(failed)?)
    }

    async fn read(
        &self,
        follower: &str,
        name: &str,
        limit: usize,
    ) -> Result<StoredEvents, ModuleError> {
        let read = self
            .events
            .read(follower, name, limit)
            .await
            .map_err(failed)?;
        Ok(StoredEvents {
            events: read
                .events
                .into_iter()
                .map(wire)
                .collect::<Result<_, _>>()?,
            fell_behind: read.fell_behind,
        })
    }

    async fn acknowledge(
        &self,
        follower: &str,
        name: &str,
        position: u64,
    ) -> Result<(), ModuleError> {
        self.events
            .acknowledge(follower, name, position)
            .await
            .map_err(failed)
    }
}

/// Drop the events older than the bus keeps them, at startup and then
/// daily, until `shutdown` is cancelled.
pub async fn drop_old_events(memory: Arc<Memory>, shutdown: CancellationToken) {
    let mut interval = tokio::time::interval(DROP_INTERVAL);
    loop {
        tokio::select! {
            () = shutdown.cancelled() => break,
            _ = interval.tick() => {}
        }
        match memory.events.drop_old(Utc::now()).await {
            Ok(dropped) => info!("Events: {dropped} older than the bus keeps dropped"),
            Err(e) => error!("Dropping old events failed: {e}"),
        }
    }
}

#[cfg(test)]
mod tests;
