// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A module process's models: `pcx.module.v1.Model` on its callback
//! socket. Dropping an answer closes its stream, which cancels the call.

use async_trait::async_trait;
use parking_lot::RwLock;
use post_cortex_proto::module as wire;
use post_cortex_proto::module::model_client::ModelClient;
use tokio::sync::mpsc;
use tonic::transport::Channel;

use crate::model::{
    ModelEvent, ModelFailure, ModelFailureKind, ModelInfo, ModelProvider, ModelRequest, ModelRun,
    checked,
};
use crate::{ModuleError, Scope};

/// Events of a remote answer buffered before the reader waits.
const QUEUE: usize = 64;

/// The models behind the host's callback socket.
pub struct RemoteModels {
    client: ModelClient<Channel>,
    /// As it last listed them: at launch, once it started, and once the
    /// listing it began then is in.
    models: RwLock<Vec<ModelInfo>>,
}

impl RemoteModels {
    /// The models over `channel`, listed at launch.
    ///
    /// # Errors
    /// The host refuses this module models, or cannot be reached.
    pub async fn connect(channel: Channel) -> Result<Self, ModuleError> {
        let client = ModelClient::new(channel);
        let models = list(client.clone(), false).await?;
        Ok(Self {
            client,
            models: RwLock::new(models),
        })
    }

    /// List its models again: a provider has its settings once started,
    /// which its listing at launch did not have. `settled`: once the
    /// listing of its API it began at start is in.
    ///
    /// # Errors
    /// As [`RemoteModels::connect`]; its listing was refused (its key);
    /// the models listed before stay.
    pub async fn refresh(&self, settled: bool) -> Result<(), ModuleError> {
        let models = list(self.client.clone(), settled).await?;
        *self.models.write() = models;
        Ok(())
    }
}

/// The models `client` lists, checked; `settled` as in [`RemoteModels::refresh`].
async fn list(
    mut client: ModelClient<Channel>,
    settled: bool,
) -> Result<Vec<ModelInfo>, ModuleError> {
    let listed = client
        .models(wire::ModelsRequest { settled })
        .await?
        .into_inner();
    if let Some(error) = listed.error {
        return Err(error.into());
    }
    Ok(checked(listed.models)?)
}

fn unreachable(status: &tonic::Status) -> ModelFailure {
    ModelFailure::new(ModelFailureKind::Unavailable, status.message().to_string())
}

#[async_trait]
impl ModelProvider for RemoteModels {
    fn models(&self) -> Vec<ModelInfo> {
        self.models.read().clone()
    }

    /// Its models once the listing its start began is in, kept as its
    /// models from now on.
    async fn listed(&self) -> Result<Vec<ModelInfo>, ModuleError> {
        self.refresh(true).await?;
        Ok(self.models())
    }

    async fn generate(
        &self,
        scope: &Scope,
        request: ModelRequest,
    ) -> Result<ModelRun, ModelFailure> {
        let mut call = tonic::Request::new(request.to_wire(scope.to_wire())?);
        call.set_timeout(scope.remaining());
        let mut replies = self
            .client
            .clone()
            .generate(call)
            .await
            .map_err(|s| unreachable(&s))?
            .into_inner();
        let (tx, rx) = mpsc::channel(QUEUE);
        let cancel = scope.cancel.clone();
        tokio::spawn(async move {
            loop {
                let next = tokio::select! {
                    () = cancel.cancelled() => return,
                    next = replies.message() => next,
                };
                let event = match next {
                    Ok(Some(reply)) => ModelEvent::from_wire(reply),
                    Ok(None) => return,
                    Err(status) => Err(unreachable(&status)),
                };
                let last = !matches!(
                    event,
                    Ok(ModelEvent::TextDelta(_)
                        | ModelEvent::ThinkingDelta(_)
                        | ModelEvent::ToolUse { .. })
                );
                if tx.send(event).await.is_err() || last {
                    return;
                }
            }
        });
        Ok(ModelRun::new(rx))
    }
}
