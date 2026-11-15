// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The organisation's bus (docs/host.md, events of a module): a module
//! emits the events its manifest declares (`Manifest.emits`) and follows
//! those it names (`Manifest.follows`), at least once, acknowledging each
//! name's events by position. A module reaches it through its handle
//! (the host's handle; a process module through `pcx.module.v1.Bus` on
//! its callback socket, [`RemoteBus`]).

use std::collections::HashSet;

use async_trait::async_trait;
use pbjson_types::Any;
use post_cortex_proto::module::BusEvent;
use tokio::sync::mpsc;

use crate::{Manifest, ModuleError, Scope};

mod remote;

pub use remote::RemoteBus;

/// What a follower is told.
#[derive(Debug, Clone, PartialEq)]
pub enum Followed {
    /// An event of a name it follows.
    Event(BusEvent),
    /// Events of this name it had not acknowledged were dropped: it
    /// re-reads what it follows.
    FellBehind(String),
}

/// The events a module follows, as they come; dropping it stops them.
pub struct EventFeed {
    replies: mpsc::Receiver<Result<Followed, ModuleError>>,
}

impl EventFeed {
    /// A feed of what `replies` brings.
    #[must_use]
    pub fn new(replies: mpsc::Receiver<Result<Followed, ModuleError>>) -> Self {
        Self { replies }
    }

    /// The next reply; none once the host stops the feed.
    pub async fn next(&mut self) -> Option<Result<Followed, ModuleError>> {
        self.replies.recv().await
    }
}

/// The bus as one module sees it.
#[async_trait]
pub trait Bus: Send + Sync {
    /// Emit the event `name` with `payload` in `scope` (its workspace and
    /// author); its id once it is kept.
    ///
    /// # Errors
    /// `InvalidArgument` for a name the module does not declare or a
    /// payload of another type; `FailedPrecondition` on a host that keeps
    /// no events; storage failed.
    async fn publish(&self, scope: &Scope, name: &str, payload: Any)
    -> Result<String, ModuleError>;

    /// The events the module follows after what it acknowledged, oldest
    /// first, then each new one.
    ///
    /// # Errors
    /// `FailedPrecondition` on a host that keeps no events.
    async fn follow(&self) -> Result<EventFeed, ModuleError>;

    /// The module has handled the events named `name` up to `position`.
    ///
    /// # Errors
    /// `InvalidArgument` for a name it does not follow; storage failed.
    async fn ack(&self, name: &str, position: u64) -> Result<(), ModuleError>;
}

/// Whether `local`, an event's name after its module's id, is one: words
/// of a-z, 0-9 and `_`, joined by `.`.
fn local_name(local: &str) -> bool {
    !local.is_empty()
        && local.split('.').all(|word| {
            !word.is_empty()
                && word
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        })
}

/// The end of a name in `Manifest.follows` that follows every event of a
/// module (`board.*`, docs/host.md, events).
pub const EVERY_EVENT: &str = ".*";

/// The events `manifest` follows, its names in `follows` read against
/// `running`, the manifests of the modules that run: a name a module it
/// uses emits, `<module>.*`, every event that module declares, or `*`,
/// every event of every module it uses (the one reading the host and the
/// cloud make).
///
/// # Errors
/// A name (or a module of `<module>.*`) no module it uses emits.
pub fn followed(manifest: &Manifest, running: &[&Manifest]) -> Result<Vec<String>, String> {
    let used = |id: &str| crate::uses(&manifest.uses, id);
    let mut names = Vec::new();
    for name in &manifest.follows {
        // `*`: every event of every module it uses.
        if name == crate::EVERY_MODULE {
            names.extend(
                running
                    .iter()
                    .filter(|m| m.id != manifest.id && used(&m.id))
                    .flat_map(|m| m.emits.iter().map(|e| e.name.clone())),
            );
            continue;
        }
        match name.strip_suffix(EVERY_EVENT) {
            Some(module) => {
                let other = running
                    .iter()
                    .find(|m| m.id == module && used(module))
                    .ok_or_else(|| format!("it follows {name}, and it uses no module {module}"))?;
                names.extend(other.emits.iter().map(|e| e.name.clone()));
            }
            None => {
                let emitted = running
                    .iter()
                    .any(|m| used(&m.id) && m.emits.iter().any(|e| &e.name == name));
                if !emitted {
                    return Err(format!("it follows {name}, which no module it uses emits"));
                }
                names.push(name.clone());
            }
        }
    }
    names.sort();
    names.dedup();
    Ok(names)
}

/// Check the events `manifest` declares: each named `<its id>.<name>`,
/// once, of a message its descriptors hold (the one check the host and the
/// cloud run).
///
/// # Errors
/// Why the manifest is not one, naming the event.
pub fn check_emits(manifest: &Manifest) -> Result<(), String> {
    if manifest.emits.is_empty() {
        return Ok(());
    }
    let pool = crate::descriptors(&manifest.descriptors)?;
    let mut seen = HashSet::new();
    for emitted in &manifest.emits {
        let name = &emitted.name;
        let local = name
            .strip_prefix(manifest.id.as_str())
            .and_then(|rest| rest.strip_prefix('.'));
        if !local.is_some_and(local_name) {
            return Err(format!(
                "its event {name} is not {}.<name> of a-z, 0-9, '_' and '.'",
                manifest.id
            ));
        }
        if !seen.insert(name.as_str()) {
            return Err(format!("it declares its event {name} twice"));
        }
        if pool.get_message_by_name(&emitted.r#type).is_none() {
            return Err(format!(
                "its event {name} is of {}, which its descriptors do not hold",
                emitted.r#type
            ));
        }
    }
    Ok(())
}

/// The full name of the message `payload` holds, as its `type_url` names it
/// (`type.googleapis.com/<type>`, or any prefix ending in `/`); none when
/// it names none.
#[must_use]
pub fn payload_type(payload: &Any) -> Option<&str> {
    payload.type_url.rsplit_once('/').map(|(_, named)| named)
}

/// Whether `payload` is a message of `type_name`.
#[must_use]
pub fn payload_is(payload: &Any, type_name: &str) -> bool {
    payload_type(payload) == Some(type_name)
}

#[cfg(test)]
mod tests;
