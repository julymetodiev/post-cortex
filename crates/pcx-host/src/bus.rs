// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The organisation's bus (docs/host.md, events of a module): the host
//! checks an event against its module's manifest, stamps it (its id, the
//! module, the scope's workspace and author, the time) and keeps it through
//! its [`EventStore`] port before the publish answers; it feeds each
//! follower the events it names after what it acknowledged, then each new
//! one, at least once.

use std::collections::HashMap;
use std::sync::{Arc, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use arc_swap::ArcSwap;
use async_trait::async_trait;
use pbjson_types::{Any, Timestamp};
use pcx_module_api::bus::{Bus, EventFeed, Followed, payload_is};
use pcx_module_api::{Manifest, ModuleError, ModuleErrorKind, Scope};
use post_cortex_proto::module::BusEvent;
use tokio::sync::{Notify, broadcast, mpsc};
use uuid::Uuid;

use crate::Loaded;

/// Events read of one name at a time.
const BATCH: usize = 64;

/// How long a follower waits for a new event before it reads again (a
/// publish wakes it sooner).
const POLL: Duration = Duration::from_secs(5);

/// Replies of a feed sent before the feed waits.
const QUEUE: usize = 64;

/// Names of published events held for the composition.
const PUBLISHED_BACKLOG: usize = 64;

/// What a follower reads of one name.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredEvents {
    /// The events after its cursor, oldest first.
    pub events: Vec<BusEvent>,
    /// Events of the name it had not acknowledged were dropped.
    pub fell_behind: bool,
}

/// Where a host keeps its organisation's events; the composition fills it
/// (memory's Events, so the tenant's one writer keeps them).
#[async_trait]
pub trait EventStore: Send + Sync {
    /// Keep `event` at the log's next position; it, with its position.
    ///
    /// # Errors
    /// Storage failed.
    async fn append(&self, event: BusEvent) -> Result<BusEvent, ModuleError>;

    /// At most `limit` events named `name` after `follower`'s cursor.
    ///
    /// # Errors
    /// Storage failed.
    async fn read(
        &self,
        follower: &str,
        name: &str,
        limit: usize,
    ) -> Result<StoredEvents, ModuleError>;

    /// `follower` has handled the events named `name` up to `position`.
    ///
    /// # Errors
    /// Storage failed.
    async fn acknowledge(
        &self,
        follower: &str,
        name: &str,
        position: u64,
    ) -> Result<(), ModuleError>;
}

/// The host's bus: its store and the wake-up of its followers.
#[derive(Default)]
pub struct HostBus {
    store: Option<Arc<dyn EventStore>>,
    appended: Notify,
    published: Option<broadcast::Sender<String>>,
}

impl HostBus {
    /// The bus over `store`; none: the host keeps no events.
    pub fn new(store: Option<Arc<dyn EventStore>>) -> Self {
        Self {
            store,
            appended: Notify::new(),
            published: Some(broadcast::channel(PUBLISHED_BACKLOG).0),
        }
    }

    /// The names of the events published from here on, for the composition
    /// (what it reloads on); one that falls behind reads its state again.
    #[must_use]
    pub fn published(&self) -> Option<broadcast::Receiver<String>> {
        self.published.as_ref().map(broadcast::Sender::subscribe)
    }

    fn tell(&self, name: &str) {
        if let Some(published) = &self.published
            && published.send(name.to_string()).is_err()
        {
            tracing::trace!(event = name, "nobody listens for what is published");
        }
    }

    fn store(&self) -> Result<&Arc<dyn EventStore>, ModuleError> {
        self.store.as_ref().ok_or_else(|| {
            ModuleError::new(
                ModuleErrorKind::FailedPrecondition,
                "this host keeps no events: its composition gives it no store",
            )
        })
    }
}

/// The bus as module `module` sees it.
pub struct ModuleBus {
    module: String,
    bus: Arc<HostBus>,
    loaded: Weak<ArcSwap<Loaded>>,
}

impl ModuleBus {
    /// The bus of `module` on the host whose running set is `loaded`.
    pub fn new(module: &str, bus: Arc<HostBus>, loaded: Weak<ArcSwap<Loaded>>) -> Self {
        Self {
            module: module.to_string(),
            bus,
            loaded,
        }
    }

    /// The module's manifest in the running set.
    fn manifest(&self) -> Result<Manifest, ModuleError> {
        let loaded = self.loaded()?;
        loaded
            .modules
            .iter()
            .find(|g| g.id() == self.module)
            .map(|g| g.module().manifest().clone())
            .ok_or_else(|| self.not_running())
    }

    /// The events the module follows, its `<module>.*` read against the
    /// modules that run.
    fn followed(&self) -> Result<Vec<String>, ModuleError> {
        let loaded = self.loaded()?;
        let running: Vec<&Manifest> = loaded
            .modules
            .iter()
            .map(|g| g.module().manifest())
            .collect();
        let manifest = running
            .iter()
            .find(|m| m.id == self.module)
            .ok_or_else(|| self.not_running())?;
        pcx_module_api::bus::followed(manifest, &running).map_err(invalid)
    }

    fn loaded(&self) -> Result<Arc<Loaded>, ModuleError> {
        Ok(self
            .loaded
            .upgrade()
            .ok_or_else(|| ModuleError::new(ModuleErrorKind::Unavailable, "the host has stopped"))?
            .load_full())
    }

    fn not_running(&self) -> ModuleError {
        ModuleError::new(
            ModuleErrorKind::FailedPrecondition,
            format!("module {} is not running", self.module),
        )
    }
}

fn invalid(message: String) -> ModuleError {
    ModuleError::new(ModuleErrorKind::InvalidArgument, message)
}

/// Now, on the wire.
///
/// # Errors
/// `Internal` for a clock before 1970 or past the wire's range.
fn now() -> Result<Timestamp, ModuleError> {
    let clock = |reason: &str| {
        ModuleError::new(
            ModuleErrorKind::Internal,
            format!("the host's clock {reason}"),
        )
    };
    let since = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| clock("is before 1970"))?;
    Ok(Timestamp {
        seconds: i64::try_from(since.as_secs()).map_err(|_| clock("is past the wire's range"))?,
        // Under a second: fits.
        nanos: i32::try_from(since.subsec_nanos()).map_err(|_| clock("names no nanosecond"))?,
    })
}

#[async_trait]
impl Bus for ModuleBus {
    async fn publish(
        &self,
        scope: &Scope,
        name: &str,
        payload: Any,
    ) -> Result<String, ModuleError> {
        let store = self.bus.store()?;
        let manifest = self.manifest()?;
        let declared = manifest
            .emits
            .iter()
            .find(|e| e.name == name)
            .ok_or_else(|| invalid(format!("module {} declares no event {name}", self.module)))?;
        if !payload_is(&payload, &declared.r#type) {
            return Err(invalid(format!(
                "event {name} is of {}, not {}",
                declared.r#type, payload.type_url
            )));
        }
        let id = Uuid::now_v7().to_string();
        store
            .append(BusEvent {
                id: id.clone(),
                position: 0,
                name: name.to_string(),
                module: self.module.clone(),
                workspace_id: scope.workspace_id.clone().unwrap_or_default(),
                author: scope.author.clone().unwrap_or_default(),
                at: Some(now()?),
                payload: Some(payload),
            })
            .await?;
        self.bus.appended.notify_waiters();
        self.bus.tell(name);
        Ok(id)
    }

    async fn follow(&self) -> Result<EventFeed, ModuleError> {
        let store = Arc::clone(self.bus.store()?);
        let names = self.followed()?;
        let (tx, rx) = mpsc::channel(QUEUE);
        let (bus, follower) = (Arc::clone(&self.bus), self.module.clone());
        tokio::spawn(async move {
            // What this feed sent of each name: an event not acknowledged is
            // sent again by the next feed, not by this one.
            let mut sent: HashMap<String, u64> = HashMap::new();
            let mut told: Vec<String> = Vec::new();
            loop {
                let woken = bus.appended.notified();
                tokio::pin!(woken);
                woken.as_mut().enable();
                let mut any = false;
                for name in &names {
                    let read = match store.read(&follower, name, BATCH).await {
                        Ok(read) => read,
                        Err(error) => {
                            // The feed ends with its failure; the follower
                            // follows again.
                            if let Err(left) = tx.send(Err(error)).await {
                                tracing::debug!(follower, "the follower left: {left}");
                            }
                            return;
                        }
                    };
                    if read.fell_behind && !told.contains(name) {
                        told.push(name.clone());
                        if tx
                            .send(Ok(Followed::FellBehind(name.clone())))
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                    let last = sent.get(name).copied().unwrap_or(0);
                    for event in read.events.into_iter().filter(|e| e.position > last) {
                        sent.insert(name.clone(), event.position);
                        any = true;
                        if tx.send(Ok(Followed::Event(event))).await.is_err() {
                            return;
                        }
                    }
                }
                if !any {
                    tokio::select! {
                        () = &mut woken => {}
                        () = tokio::time::sleep(POLL) => {}
                        () = tx.closed() => return,
                    }
                }
            }
        });
        Ok(EventFeed::new(rx))
    }

    async fn ack(&self, name: &str, position: u64) -> Result<(), ModuleError> {
        let store = self.bus.store()?;
        if !self.followed()?.iter().any(|f| f == name) {
            return Err(invalid(format!(
                "module {} does not follow {name}",
                self.module
            )));
        }
        store.acknowledge(&self.module, name, position).await
    }
}

#[cfg(test)]
mod tests;
