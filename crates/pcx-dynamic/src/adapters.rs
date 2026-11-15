// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The `dynamic` module's ports: memory's blobs, the board and the bus over
//! its host, and the launcher of `pcx-modules` over its socket.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use pcx_host::{Grant, Launch, ModuleLauncher, ProcessModule, ProcessSpec};
use pcx_module_api::grpc::GrpcChannel;
use pcx_module_api::{ModuleError, ModuleErrorKind, ModuleHost, Scope};
use pcx_module_launcher::{Remote, Requested, Runtimes, prepare};
use post_cortex_proto::board::boards_client::BoardsClient;
use post_cortex_proto::board::{
    AddTaskRequest, DecideRequest, DecisionRecord, GetTaskRequest, TaskRecord, Thread,
    WithdrawDecisionRequest,
};
use post_cortex_proto::dynamic::{Check, Grants, VersionApproved, VersionRemoved};
use post_cortex_proto::module::ModuleKind;
use post_cortex_proto::pb::blobs_client::BlobsClient;
use post_cortex_proto::pb::{Attachment, BlobRequest, PutBlobChunk};
use prost::{Message, Name};
use tokio_util::sync::CancellationToken;

use crate::domain::MODULE_ID;
use crate::ports::{Blobs, Board, Bundles, Checking, Events, Prepared, Reach};
use crate::specs::{VERSION_APPROVED, VERSION_REMOVED};

/// The deadline of the module's own calls.
const OWN_DEADLINE: Duration = Duration::from_secs(60);

/// How much of a blob travels at a time.
const CHUNK_BYTES: usize = 1024 * 1024;

/// The media type of a bundle.
const TAR: &str = "application/x-tar";

/// How long a locked run's tests may take (docs/modules/dynamic.md).
const LOCKED_RUN: Duration = Duration::from_secs(60);

/// A locked run's limit of open files.
const LOCKED_FILES: u32 = 256;

/// The last of what a locked run wrote that a person is shown.
const LOG_LINES: usize = 40;

/// What the Python binding writes when a package turned the GIL on.
const GIL_ON: &str = "pcx_module: the GIL is on";

/// Locked runs started by this process.
static CHECKS: AtomicU64 = AtomicU64::new(0);

/// The ports over the module's host.
#[derive(Clone)]
pub struct HostPorts {
    /// The module's handle.
    pub host: Arc<dyn ModuleHost>,
    /// Its channel.
    pub channel: GrpcChannel,
    /// Its workspace.
    pub workspace: String,
}

impl HostPorts {
    fn own(&self, author: &str) -> Scope {
        Scope::own(
            author,
            &self.workspace,
            OWN_DEADLINE,
            CancellationToken::new(),
        )
    }

    fn request<T>(&self, author: &str, message: T) -> Result<tonic::Request<T>, ModuleError> {
        let mut request = tonic::Request::new(message);
        self.own(author).to_metadata(request.metadata_mut())?;
        Ok(request)
    }
}

#[async_trait]
impl Blobs for HostPorts {
    async fn put(&self, bytes: Vec<u8>) -> Result<Attachment, ModuleError> {
        let size = bytes.len() as u64;
        let chunks: Vec<PutBlobChunk> = bytes
            .chunks(CHUNK_BYTES)
            .enumerate()
            .map(|(i, data)| PutBlobChunk {
                workspace_id: if i == 0 {
                    self.workspace.clone()
                } else {
                    String::new()
                },
                data: data.to_vec(),
                media_type: if i == 0 {
                    TAR.to_string()
                } else {
                    String::new()
                },
            })
            .collect();
        let info = BlobsClient::new(self.channel.clone())
            .put_blob(self.request(MODULE_ID, tokio_stream::iter(chunks))?)
            .await?
            .into_inner();
        Ok(Attachment {
            sha256: info.sha256,
            name: "bundle.tar".to_string(),
            media_type: TAR.to_string(),
            size,
            ..Default::default()
        })
    }

    async fn get(&self, sha256: &str) -> Result<Vec<u8>, ModuleError> {
        let mut chunks = BlobsClient::new(self.channel.clone())
            .get_blob(self.request(
                MODULE_ID,
                BlobRequest {
                    workspace_id: self.workspace.clone(),
                    sha256: sha256.to_string(),
                },
            )?)
            .await?
            .into_inner();
        let mut bytes = Vec::new();
        while let Some(chunk) = chunks.message().await? {
            bytes.extend_from_slice(&chunk.data);
        }
        Ok(bytes)
    }
}

#[async_trait]
impl Board for HostPorts {
    async fn task(&self, asker: &str, task: AddTaskRequest) -> Result<TaskRecord, ModuleError> {
        Ok(BoardsClient::new(self.channel.clone())
            .add_task(self.request(asker, task)?)
            .await?
            .into_inner())
    }

    async fn decide(
        &self,
        asker: &str,
        question: DecideRequest,
    ) -> Result<DecisionRecord, ModuleError> {
        Ok(BoardsClient::new(self.channel.clone())
            .decide(self.request(asker, question)?)
            .await?
            .into_inner())
    }

    async fn thread(&self, asker: &str, task: &str) -> Result<Thread, ModuleError> {
        Ok(BoardsClient::new(self.channel.clone())
            .get_task(self.request(
                asker,
                GetTaskRequest {
                    workspace_id: self.workspace.clone(),
                    task: task.to_string(),
                },
            )?)
            .await?
            .into_inner())
    }

    async fn withdraw(
        &self,
        asker: &str,
        request: WithdrawDecisionRequest,
    ) -> Result<DecisionRecord, ModuleError> {
        Ok(BoardsClient::new(self.channel.clone())
            .withdraw_decision(self.request(asker, request)?)
            .await?
            .into_inner())
    }
}

#[async_trait]
impl Reach for HostPorts {
    async fn modules(&self, scope: &Scope) -> Result<BTreeSet<String>, ModuleError> {
        Ok(self
            .host
            .tools(scope)
            .await?
            .into_iter()
            .map(|used| used.module)
            .collect())
    }
}

#[async_trait]
impl Events for HostPorts {
    async fn changed(&self, id: &str, removed: bool) {
        let (name, payload) = if removed {
            (VERSION_REMOVED, any(&VersionRemoved { id: id.to_string() }))
        } else {
            (
                VERSION_APPROVED,
                any(&VersionApproved { id: id.to_string() }),
            )
        };
        let scope = self.own(MODULE_ID);
        let published = match self.host.bus().await {
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

fn any<T: Message + Name>(message: &T) -> pbjson_types::Any {
    pbjson_types::Any {
        type_url: format!("type.googleapis.com/{}", T::full_name()),
        value: message.encode_to_vec().into(),
    }
}

/// The launcher of a host's dynamic modules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LauncherConfig {
    /// Its socket.
    pub socket: PathBuf,
    /// The directory of the launches' sockets, shared with it.
    pub sockets: PathBuf,
    /// The runtimes, as it reads them.
    pub runtimes: Runtimes,
}

/// What prepares and checks bundles: the launcher.
pub struct Launcher {
    /// Its configuration.
    pub config: LauncherConfig,
    /// Whose scratch directories a check uses: the module's workspace.
    pub owner: String,
}

fn failed(reason: impl Into<String>) -> ModuleError {
    ModuleError::new(ModuleErrorKind::FailedPrecondition, reason)
}

/// The grants of a module as the host reads them.
#[must_use]
pub fn host_grants(grants: &Grants) -> Vec<Grant> {
    let mut found = Vec::new();
    if grants.write_memory {
        found.push(Grant::WriteMemory);
    }
    if grants.read_conversation {
        found.push(Grant::ReadConversation);
    }
    if grants.models {
        found.push(Grant::Models);
    }
    found
}

#[async_trait]
impl Bundles for Launcher {
    async fn prepare(&self, bytes: Vec<u8>, sha256: &str) -> Result<Prepared, ModuleError> {
        let prepared = prepare(&self.config.socket, bytes, sha256)
            .await
            .map_err(failed)?;
        Ok(Prepared {
            bundle: prepared.bundle.into(),
            environment: (!prepared.environment.is_empty()).then(|| prepared.environment.into()),
        })
    }

    async fn check(
        &self,
        checking: Checking<'_>,
        prepared: &Prepared,
    ) -> Result<Check, ModuleError> {
        let Checking {
            id,
            language,
            grants,
            has_tests,
            memory_mb,
        } = checking;
        let environment = prepared.environment.as_deref();
        let run = self
            .config
            .runtimes
            .module(language, &prepared.bundle, environment, memory_mb)
            .map_err(failed)?;
        let requested = |run: &pcx_module_launcher::Run| Requested {
            owner: self.owner.clone(),
            read: run.read.clone(),
            cwd: run.cwd.clone(),
            env: run.env.clone(),
            memory_mb,
            open_files: LOCKED_FILES,
            sha256: String::new(),
        };
        let spec = ProcessSpec {
            id: id.to_string(),
            command: run.program.clone(),
            args: run.args.clone(),
            sha256: String::new(),
            config: None,
            grants: host_grants(grants),
            uses: grants.uses.clone(),
            filesystem: Vec::new(),
        };
        let launcher: Arc<dyn ModuleLauncher> =
            Arc::new(Remote::new(self.config.socket.clone(), requested(&run)));
        let (manifest, output) = ProcessModule::describe_by(&spec, &self.config.sockets, launcher)
            .await
            .map_err(|e| failed(e.to_string()))?;
        if manifest.kind != i32::from(ModuleKind::Tool) {
            return Err(failed(format!("{id}: a dynamic module's kind is tool")));
        }
        if !manifest.config_type.is_empty() {
            return Err(failed(format!(
                "{id}: a dynamic module takes no settings; it keeps its parameters in its documents"
            )));
        }
        let mut lines: Vec<String> = output.into_iter().map(|l| l.text).collect();
        let gil = lines.iter().any(|l| l.contains(GIL_ON));
        // The compiler's memory is not the module's: its tests' types are
        // checked apart, within a limit of their own, and they run without them.
        let mut tests_passed = true;
        if has_tests {
            if let Some(types) = self
                .config
                .runtimes
                .types(language, &prepared.bundle, environment)
                .map_err(failed)?
            {
                let asked = Requested {
                    memory_mb: TYPES_MEMORY_MB,
                    ..requested(&types)
                };
                let (passed, written) = self.locked(id, &asked, &types).await?;
                lines.extend(written);
                tests_passed = passed;
            }
            if tests_passed {
                let tests = self
                    .config
                    .runtimes
                    .tests(language, &prepared.bundle, environment)
                    .map_err(failed)?;
                let (passed, written) = self.locked(id, &requested(&tests), &tests).await?;
                lines.extend(written);
                tests_passed = passed;
            }
        }
        let from = lines.len().saturating_sub(LOG_LINES);
        Ok(Check {
            tools: manifest
                .rpcs
                .iter()
                .filter(|r| !r.tool.is_empty())
                .map(|r| r.tool.clone())
                .collect(),
            uses: manifest.uses.clone(),
            tests_passed,
            log: lines[from..].join("\n"),
            gil,
        })
    }
}

impl Launcher {
    /// `run` to its end through the launcher: whether it passed, and what it
    /// wrote. It serves nothing: a socket's directory is only its own.
    async fn locked(
        &self,
        id: &str,
        requested: &Requested,
        run: &pcx_module_launcher::Run,
    ) -> Result<(bool, Vec<String>), ModuleError> {
        let dir = tempdir_in(&self.config.sockets, id)?;
        let launcher = Remote::new(self.config.socket.clone(), requested.clone());
        let launched = launcher
            .launch(&Launch {
                id: id.to_string(),
                command: run.program.clone(),
                args: run.args.clone(),
                socket: dir.join("m.sock"),
            })
            .await
            .map_err(failed);
        let result = match launched {
            Ok(mut process) => {
                let mut lines_of = process.lines();
                let end = tokio::time::timeout(LOCKED_RUN, process.wait()).await;
                let mut written = Vec::new();
                if let Some(lines) = &mut lines_of {
                    while let Ok(line) = lines.try_recv() {
                        written.push(line);
                    }
                }
                match end {
                    Ok(end) => {
                        written.push(format!("its check {}", end.how));
                        Ok((end.success, written))
                    }
                    Err(_) => {
                        written.push(format!("its check did not end within {LOCKED_RUN:?}"));
                        Ok((false, written))
                    }
                }
            }
            Err(refused) => Err(refused),
        };
        if let Err(e) = std::fs::remove_dir_all(&dir) {
            tracing::warn!(error = %e, "remove a locked run's directory");
        }
        result
    }
}

/// What checking a TypeScript bundle's types may take, in MB: measured 419 MB
/// with `marked`, where its tests alone took 196 MB.
const TYPES_MEMORY_MB: u32 = 1024;

/// A new directory of `id`'s under `parent`, which hosts of several tenants
/// may share, each with a module of that name.
fn tempdir_in(parent: &Path, id: &str) -> Result<PathBuf, ModuleError> {
    let check = CHECKS.fetch_add(1, Ordering::Relaxed);
    let dir = parent.join(format!("check-{}-{check}-{id}", std::process::id()));
    std::fs::create_dir(&dir).map_err(|e| failed(format!("{}: {e}", dir.display())))?;
    Ok(dir)
}
