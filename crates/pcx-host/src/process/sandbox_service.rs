// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! `pcx.module.v1.Sandbox` on a module process's callback socket: its
//! files and processes, through its handle on the host, so under its
//! policy. Closing an `Exec` stream kills the process.

use std::sync::Arc;

use pcx_module_api::sandbox::{ExecEvent, ExecSpec, Sandbox, SandboxPath};
use pcx_module_api::{ModuleError, ModuleHost};
use post_cortex_proto::module as wire;
use post_cortex_proto::module::sandbox_server::Sandbox as SandboxService;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;
use tonic::{Request, Response, Status};

/// Replies of a run sent before the stream waits.
const EXEC_QUEUE: usize = 64;

/// The module's handle behind the generated service.
pub struct Served {
    host: Arc<dyn ModuleHost>,
}

impl Served {
    /// The service over `host`, the module's handle.
    pub fn new(host: Arc<dyn ModuleHost>) -> Self {
        Self { host }
    }

    async fn sandbox(&self) -> Result<Arc<dyn Sandbox>, ModuleError> {
        self.host.sandbox().await
    }
}

/// A wire answer of a call without a value.
fn answer(result: Result<(), ModuleError>) -> wire::SandboxResponse {
    wire::SandboxResponse {
        error: result.err().map(wire::ModuleError::from),
    }
}

#[tonic::async_trait]
impl SandboxService for Served {
    async fn policy(
        &self,
        _request: Request<wire::SandboxPolicyRequest>,
    ) -> Result<Response<wire::SandboxPolicyResponse>, Status> {
        let policy = self.sandbox().await.and_then(|s| s.policy().to_wire());
        Ok(Response::new(match policy {
            Ok(policy) => policy,
            Err(error) => wire::SandboxPolicyResponse {
                error: Some(error.into()),
                ..wire::SandboxPolicyResponse::default()
            },
        }))
    }

    async fn read(
        &self,
        request: Request<wire::ReadFileRequest>,
    ) -> Result<Response<wire::ReadFileResponse>, Status> {
        let request = request.into_inner();
        let read = async {
            let file = SandboxPath::required(request.file, "file")?;
            let limit = usize::try_from(request.limit).unwrap_or(usize::MAX);
            self.sandbox().await?.read(&file, limit).await
        };
        let result = match read.await {
            Ok(data) => wire::read_file_response::Result::Data(data),
            Err(error) => wire::read_file_response::Result::Error(error.into()),
        };
        Ok(Response::new(wire::ReadFileResponse {
            result: Some(result),
        }))
    }

    async fn write(
        &self,
        request: Request<wire::WriteFileRequest>,
    ) -> Result<Response<wire::SandboxResponse>, Status> {
        let request = request.into_inner();
        let write = async {
            let file = SandboxPath::required(request.file, "file")?;
            self.sandbox().await?.write(&file, request.data).await
        };
        Ok(Response::new(answer(write.await)))
    }

    async fn list(
        &self,
        request: Request<wire::ListDirRequest>,
    ) -> Result<Response<wire::ListDirResponse>, Status> {
        let request = request.into_inner();
        let list = async {
            let dir = SandboxPath::required(request.dir, "dir")?;
            self.sandbox().await?.list(&dir).await
        };
        Ok(Response::new(match list.await {
            Ok(entries) => wire::ListDirResponse {
                entries: entries.iter().map(|e| e.to_wire()).collect(),
                error: None,
            },
            Err(error) => wire::ListDirResponse {
                entries: Vec::new(),
                error: Some(error.into()),
            },
        }))
    }

    async fn stat(
        &self,
        request: Request<wire::StatRequest>,
    ) -> Result<Response<wire::StatResponse>, Status> {
        let request = request.into_inner();
        let stat = async {
            let path = SandboxPath::required(request.path, "path")?;
            self.sandbox().await?.metadata(&path).await
        };
        Ok(Response::new(match stat.await {
            Ok(metadata) => wire::StatResponse {
                metadata: Some(metadata.to_wire()),
                error: None,
            },
            Err(error) => wire::StatResponse {
                metadata: None,
                error: Some(error.into()),
            },
        }))
    }

    async fn remove(
        &self,
        request: Request<wire::RemoveRequest>,
    ) -> Result<Response<wire::SandboxResponse>, Status> {
        let request = request.into_inner();
        let remove = async {
            let path = SandboxPath::required(request.path, "path")?;
            self.sandbox().await?.remove(&path).await
        };
        Ok(Response::new(answer(remove.await)))
    }

    async fn rename(
        &self,
        request: Request<wire::RenameRequest>,
    ) -> Result<Response<wire::SandboxResponse>, Status> {
        let request = request.into_inner();
        let rename = async {
            let from = SandboxPath::required(request.from, "from")?;
            let to = SandboxPath::required(request.to, "to")?;
            self.sandbox().await?.rename(&from, &to).await
        };
        Ok(Response::new(answer(rename.await)))
    }

    type ExecStream = ReceiverStream<Result<wire::ExecReply, Status>>;

    async fn exec(
        &self,
        request: Request<wire::ExecRequest>,
    ) -> Result<Response<Self::ExecStream>, Status> {
        let (tx, rx) = mpsc::channel(EXEC_QUEUE);
        let started = async {
            let spec = ExecSpec::from_wire(request.into_inner())?;
            let cancel = CancellationToken::new();
            let run = self.sandbox().await?.exec(spec, cancel.clone()).await?;
            Ok::<_, ModuleError>((run, cancel))
        };
        match started.await {
            Ok((mut run, cancel)) => {
                tokio::spawn(async move {
                    // The caller closing the stream kills the process.
                    let _kill_on_close = cancel.drop_guard();
                    while let Some(event) = run.next().await {
                        let last = matches!(event, ExecEvent::Exit(_));
                        if tx.send(Ok(event.to_wire())).await.is_err() || last {
                            return;
                        }
                    }
                });
            }
            Err(error) => {
                let reply = wire::ExecReply {
                    event: Some(wire::exec_reply::Event::Error(error.into())),
                };
                if tx.send(Ok(reply)).await.is_err() {
                    tracing::debug!("an exec stream closed before its error");
                }
            }
        }
        Ok(Response::new(ReceiverStream::new(rx)))
    }
}
