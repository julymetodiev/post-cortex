// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A module process's sandbox: `pcx.module.v1.Sandbox` on its callback
//! socket, under the policy the host binds to the module.

use async_trait::async_trait;
use post_cortex_proto::module as wire;
use post_cortex_proto::module::sandbox_client::SandboxClient;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tonic::transport::Channel;

use crate::sandbox::{
    DirEntry, ExecEvent, ExecExit, ExecRun, ExecSpec, Metadata, Policy, Sandbox, SandboxPath,
};
use crate::{ModuleError, ModuleErrorKind};

/// Events of a remote run buffered before the reader waits.
const EXEC_QUEUE: usize = 64;

/// The sandbox behind the host's callback socket.
pub struct RemoteSandbox {
    client: SandboxClient<Channel>,
    policy: Policy,
}

impl RemoteSandbox {
    /// The sandbox over `channel`, its policy asked of the host.
    ///
    /// # Errors
    /// The host has no sandbox, or cannot be reached.
    pub async fn connect(channel: Channel) -> Result<Self, ModuleError> {
        let mut client = SandboxClient::new(channel);
        let answer = client
            .policy(wire::SandboxPolicyRequest {})
            .await?
            .into_inner();
        Ok(Self {
            client,
            policy: Policy::from_wire(answer)?,
        })
    }
}

/// The call's error, if it failed.
fn done(response: wire::SandboxResponse) -> Result<(), ModuleError> {
    match response.error {
        Some(error) => Err(error.into()),
        None => Ok(()),
    }
}

#[async_trait]
impl Sandbox for RemoteSandbox {
    fn policy(&self) -> &Policy {
        &self.policy
    }

    async fn read(&self, file: &SandboxPath, limit: usize) -> Result<Vec<u8>, ModuleError> {
        let request = wire::ReadFileRequest {
            file: Some(file.to_wire()?),
            limit: u64::try_from(limit).unwrap_or(u64::MAX),
        };
        match self.client.clone().read(request).await?.into_inner().result {
            Some(wire::read_file_response::Result::Data(data)) => Ok(data),
            Some(wire::read_file_response::Result::Error(error)) => Err(error.into()),
            None => Err(ModuleError::new(
                ModuleErrorKind::Internal,
                "Read answered without data or an error",
            )),
        }
    }

    async fn write(&self, file: &SandboxPath, bytes: Vec<u8>) -> Result<(), ModuleError> {
        let request = wire::WriteFileRequest {
            file: Some(file.to_wire()?),
            data: bytes,
        };
        done(self.client.clone().write(request).await?.into_inner())
    }

    async fn list(&self, dir: &SandboxPath) -> Result<Vec<DirEntry>, ModuleError> {
        let request = wire::ListDirRequest {
            dir: Some(dir.to_wire()?),
        };
        let answer = self.client.clone().list(request).await?.into_inner();
        if let Some(error) = answer.error {
            return Err(error.into());
        }
        answer
            .entries
            .into_iter()
            .map(DirEntry::from_wire)
            .collect()
    }

    async fn metadata(&self, path: &SandboxPath) -> Result<Metadata, ModuleError> {
        let request = wire::StatRequest {
            path: Some(path.to_wire()?),
        };
        let answer = self.client.clone().stat(request).await?.into_inner();
        if let Some(error) = answer.error {
            return Err(error.into());
        }
        Metadata::required(answer.metadata)
    }

    async fn remove(&self, path: &SandboxPath) -> Result<(), ModuleError> {
        let request = wire::RemoveRequest {
            path: Some(path.to_wire()?),
        };
        done(self.client.clone().remove(request).await?.into_inner())
    }

    async fn rename(&self, from: &SandboxPath, to: &SandboxPath) -> Result<(), ModuleError> {
        let request = wire::RenameRequest {
            from: Some(from.to_wire()?),
            to: Some(to.to_wire()?),
        };
        done(self.client.clone().rename(request).await?.into_inner())
    }

    /// Cancelling drops the stream, which makes the host kill the process.
    async fn exec(
        &self,
        spec: ExecSpec,
        cancel: CancellationToken,
    ) -> Result<ExecRun, ModuleError> {
        let mut replies = self
            .client
            .clone()
            .exec(spec.to_wire()?)
            .await?
            .into_inner();
        let (tx, rx) = mpsc::channel(EXEC_QUEUE);
        tokio::spawn(async move {
            loop {
                let reply = tokio::select! {
                    () = cancel.cancelled() => {
                        // The reader may be gone too: then nobody waits.
                        if tx.send(ExecEvent::Exit(ExecExit::Cancelled)).await.is_err() {
                            tracing::debug!("a cancelled run's reader is gone");
                        }
                        return;
                    }
                    reply = replies.message() => reply,
                };
                let event = match reply {
                    Ok(Some(reply)) => ExecEvent::from_wire(reply),
                    // The stream ended without an exit: the reader sees no
                    // exit (ExecRun::collect reports it).
                    Ok(None) => return,
                    Err(status) => Err(ModuleError::from(status)),
                };
                let event = match event {
                    Ok(event) => event,
                    Err(error) => {
                        tracing::warn!(error = %error, "a sandboxed run's stream failed");
                        return;
                    }
                };
                let last = matches!(event, ExecEvent::Exit(_));
                if tx.send(event).await.is_err() || last {
                    return;
                }
            }
        });
        Ok(ExecRun::new(rx))
    }
}
