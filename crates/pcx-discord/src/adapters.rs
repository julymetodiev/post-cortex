// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The module's ports filled: Discord's REST API and Gateway, and the bus
//! of the module's host, where what it hears is published in the workspace
//! its settings name, as the module.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use pbjson_types::Any;
use pcx_module_api::{ModuleError, ModuleErrorKind, ModuleHost, Scope};
use prost::{Message, Name};
use tokio_util::sync::CancellationToken;

use crate::domain::MODULE_ID;
use crate::domain::heard::{AUTOMOD_ACTED, Heard, MEMBER_JOINED, MESSAGE_CREATED, POST_CREATED};
use crate::ports::Events;

pub mod rest;
pub mod socket;

/// How long keeping an event may take.
const PUBLISH: Duration = Duration::from_secs(10);

/// `message` as an event's payload.
fn any<M: Message + Name>(message: &M) -> Any {
    Any {
        type_url: M::type_url(),
        value: message.encode_to_vec().into(),
    }
}

/// An event's name and message, as the manifest declares it.
fn on_the_bus(heard: &Heard) -> (&'static str, Any) {
    match heard {
        Heard::Post(post) => (POST_CREATED, any(post)),
        Heard::Message(message) => (MESSAGE_CREATED, any(message)),
        Heard::Joined(member) => (MEMBER_JOINED, any(member)),
        Heard::Automod(acted) => (AUTOMOD_ACTED, any(acted)),
    }
}

/// The bus of the module's host.
pub struct HostEvents {
    host: Arc<dyn ModuleHost>,
    workspace: String,
    stop: CancellationToken,
}

impl HostEvents {
    /// The bus behind `host`, for events of `workspace`, until `stop`.
    pub fn new(host: Arc<dyn ModuleHost>, workspace: &str, stop: CancellationToken) -> Self {
        Self {
            host,
            workspace: workspace.to_string(),
            stop,
        }
    }
}

#[async_trait]
impl Events for HostEvents {
    async fn publish(&self, heard: Heard) {
        let (name, payload) = on_the_bus(&heard);
        let scope = Scope::own(MODULE_ID, &self.workspace, PUBLISH, self.stop.child_token());
        let published: Result<String, ModuleError> = match self.host.bus().await {
            Ok(bus) => bus.publish(&scope, name, payload).await,
            Err(error) => Err(error),
        };
        match published {
            Ok(_) => {}
            // A host that keeps no events (a test's, a bare one).
            Err(e) if e.kind == ModuleErrorKind::FailedPrecondition => {
                tracing::debug!(event = name, "the host keeps no events: {e}");
            }
            Err(e) => tracing::warn!(event = name, "the event was not kept: {e}"),
        }
    }
}
