// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The host's link to a running module process: its client and its event
//! stream, with the events waiting for their answers.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use parking_lot::Mutex;
use pcx_module_api::{HostContext, ModuleError, ModuleErrorKind, Outcome};
use post_cortex_proto::module as wire;
use post_cortex_proto::module::module_client::ModuleClient;
use tokio::sync::{mpsc, oneshot};
use tokio_stream::wrappers::ReceiverStream;
use tonic::transport::Channel;

/// Events sent to a module before the sender waits.
const EVENT_QUEUE: usize = 64;

/// Answers awaited from the module's event stream, by `seq`.
pub type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Outcome, ModuleError>>>>>;

/// The link to the module's current process.
pub struct Link {
    client: Mutex<ModuleClient<Channel>>,
    events: Mutex<Option<mpsc::Sender<wire::EventEnvelope>>>,
    pending: Pending,
    /// What the host started it with, to start a restarted process alike.
    started_with: Mutex<Option<HostContext>>,
}

impl Link {
    /// A link over `client`, not started yet.
    pub fn new(client: ModuleClient<Channel>) -> Self {
        Self {
            client: Mutex::new(client),
            events: Mutex::new(None),
            pending: Arc::default(),
            started_with: Mutex::new(None),
        }
    }

    /// The current process's client.
    pub fn client(&self) -> ModuleClient<Channel> {
        self.client.lock().clone()
    }

    /// The events waiting for their answers.
    pub fn pending(&self) -> &Pending {
        &self.pending
    }

    /// The sender of the current event stream, if one is open.
    pub fn events(&self) -> Option<mpsc::Sender<wire::EventEnvelope>> {
        self.events.lock().clone()
    }

    /// Close the event stream (no more events go out).
    pub fn close_events(&self) {
        *self.events.lock() = None;
    }

    /// Start the process behind `client` with `ctx` and its callback socket
    /// `host_socket`: Start, and the event stream when `events`
    /// (it subscribed to any).
    ///
    /// # Errors
    /// The process refused or failed a step.
    pub async fn start(
        &self,
        client: ModuleClient<Channel>,
        ctx: HostContext,
        events: bool,
        module: &str,
        host_socket: &Path,
    ) -> Result<(), ModuleError> {
        let started = client
            .clone()
            .start(wire::StartRequest {
                host_api: Some(ctx.host_api),
                config: Some(ctx.config.clone()),
                host_socket: host_socket.display().to_string(),
            })
            .await
            .map_err(ModuleError::from)?
            .into_inner();
        if let Some(error) = started.error {
            return Err(error.into());
        }
        if events {
            let sender = open_events(&client, Arc::clone(&self.pending), module).await?;
            *self.events.lock() = Some(sender);
        }
        *self.client.lock() = client;
        *self.started_with.lock() = Some(ctx);
        Ok(())
    }

    /// What the host started the module with, once it did.
    pub fn started_with(&self) -> Option<HostContext> {
        self.started_with.lock().clone()
    }
}

/// Open an event stream on `client`; its answers reach the calls waiting
/// by `seq` in `pending`, and its end fails every one still waiting.
async fn open_events(
    client: &ModuleClient<Channel>,
    pending: Pending,
    module: &str,
) -> Result<mpsc::Sender<wire::EventEnvelope>, ModuleError> {
    let (sender, receiver) = mpsc::channel(EVENT_QUEUE);
    let mut answers = client
        .clone()
        .events(ReceiverStream::new(receiver))
        .await
        .map_err(ModuleError::from)?
        .into_inner();
    let module = module.to_string();
    tokio::spawn(async move {
        loop {
            match answers.message().await {
                Ok(Some(envelope)) => {
                    let answer = match envelope.result {
                        Some(wire::outcome_envelope::Result::Outcome(o)) => Ok(o),
                        Some(wire::outcome_envelope::Result::Error(e)) => Err(e.into()),
                        None => Err(ModuleError::new(
                            ModuleErrorKind::Internal,
                            "an answer without an outcome or an error",
                        )),
                    };
                    let waiter = pending.lock().remove(&envelope.seq);
                    // The caller gave up (its deadline): nobody waits.
                    if let Some(waiter) = waiter
                        && waiter.send(answer).is_err()
                    {
                        tracing::debug!(module, seq = envelope.seq, "late event answer");
                    }
                }
                Ok(None) => break,
                Err(status) => {
                    tracing::warn!(module, error = %status, "event stream failed");
                    break;
                }
            }
        }
        // The stream ended: every waiting event fails now, not at its
        // deadline.
        for (_, waiter) in pending.lock().drain() {
            let ended = ModuleError::new(
                ModuleErrorKind::Unavailable,
                "the module's event stream ended",
            );
            if waiter.send(Err(ended)).is_err() {
                tracing::debug!(module, "event abandoned before the stream ended");
            }
        }
    });
    Ok(sender)
}

/// Removes a waiting event when its caller stops waiting.
pub struct Waiting<'a> {
    /// The waiting events.
    pub pending: &'a Pending,
    /// This one's number.
    pub seq: u64,
}

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        self.pending.lock().remove(&self.seq);
    }
}
