// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! `MemoryModule`: memory in the host's `memory` slot (ARCHITECTURE
//! section 7). It offers the memory tools, which the host lists to every
//! transport, and answers session start, file edits and turn end.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use parking_lot::Mutex;
use pcx_module_api::grpc::GrpcChannel;
use pcx_module_api::{
    API_VERSION, Capability, Effect, Event, HostContext, Manifest, Module, ModuleError,
    ModuleErrorKind, ModuleHost, ModuleKind, Outcome, Resource, Scope, ToolOutput, ToolSpec,
};
use post_cortex_core::core::document::{DocumentKind, KindDeclaration};
use post_cortex_ports::{DocumentKinds, KindsError};
use post_cortex_proto::module::{EmittedEvent, Rpc, ToolVisibility};
use serde_json::Value;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::Memory;
use crate::reach::{self, ReachRefusal};
use crate::tools::SchemaError;
use crate::tools::announce::{self, RECORD_ADDED, RECORD_ADDED_TYPE};
use crate::tools::catalog::ToolCatalog;
use crate::tools::events;
use crate::tools::handlers::{Caller, dispatch};
use crate::tools::operations::classify;

/// How long the module's stop waits for the sessions to be written.
const FLUSH_DEADLINE: Duration = Duration::from_secs(30);
/// How long a stop waits for the warm-up to finish the session it is on.
const WARM_UP_DEADLINE: Duration = Duration::from_secs(10);

/// The memory module over one memory instance.
pub struct MemoryModule {
    memory: Arc<Memory>,
    manifest: Manifest,
    tools: ToolCatalog,
    /// The warm-up the start began and what tells it to stop, until the
    /// stop.
    warm_up: Mutex<Option<(JoinHandle<()>, CancellationToken)>>,
    /// What tells the announcement of records (`announce`) to stop, while
    /// it runs.
    announcing: Mutex<Option<CancellationToken>>,
    /// Its handle on the host, from its start: the running modules'
    /// document kinds.
    host: Handle,
    /// Its `pcx.v1` service, when the composition serves it to the other
    /// modules (W23).
    services: Option<GrpcChannel>,
}

/// Memory's handle on its host, shared with its document kinds adapter.
type Handle = Arc<Mutex<Option<Arc<dyn ModuleHost>>>>;

/// The `pcx.v1` service the module serves.
const SERVICE: &str = "pcx.v1.PostCortex";

/// Its blobs' service (W24).
const BLOBS: &str = "pcx.v1.Blobs";

/// The RPCs of `pcx.v1` the other modules call: their documents and blobs,
/// the workspaces, sessions, records, context and messages, each with its service, its
/// tool name when agents' models call it too (owner 2026-10-26: MCP
/// clients call memory's MCP tools, models its RPCs) and its effect, the
/// operation table's (a test ties them: docs/host.md, effects).
const RPCS: &[(&str, &str, &str, Effect)] = &[
    (SERVICE, "PutDocument", "", Effect::Write),
    (SERVICE, "GetDocument", "", Effect::Read),
    (SERVICE, "GetDocumentHistory", "", Effect::Read),
    (SERVICE, "ListDocuments", "", Effect::Read),
    (SERVICE, "DeleteDocument", "", Effect::Write),
    (BLOBS, "PutBlob", "", Effect::Write),
    (BLOBS, "GetBlob", "", Effect::Read),
    (BLOBS, "StatBlob", "", Effect::Read),
    (SERVICE, "ListWorkspaces", "", Effect::Read),
    (SERVICE, "CreateSession", "", Effect::Write),
    (SERVICE, "AddSessionToWorkspace", "", Effect::Write),
    (SERVICE, "UpdateContext", "memory_record", Effect::Write),
    (SERVICE, "SemanticSearch", "memory_search", Effect::Read),
    (SERVICE, "AssembleContext", "memory_context", Effect::Read),
    (SERVICE, "AppendMessages", "", Effect::Write),
    (SERVICE, "ListMessages", "", Effect::Read),
];

impl MemoryModule {
    /// The module's id.
    pub const ID: &'static str = "post-cortex.memory";

    /// The module over `memory`, its tools built once.
    ///
    /// # Errors
    /// A tool's argument schema could not be built.
    pub fn new(memory: Arc<Memory>) -> Result<Self, SchemaError> {
        Ok(Self {
            memory,
            manifest: Manifest {
                emits: Vec::new(),
                follows: Vec::new(),
                workflow_templates: Vec::new(),
                description:
                    "Records, search, the graph, documents and blobs: the workspace's memory."
                        .to_string(),
                id: Self::ID.to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                api: Some(API_VERSION),
                kind: ModuleKind::Memory.into(),
                events: events::subscriptions(),
                capabilities: vec![
                    Capability::ReadConversation.into(),
                    Capability::WriteMemory.into(),
                ],
                filesystem: Vec::new(),
                views: Vec::new(),
                document_kinds: Vec::new(),
                uses: Vec::new(),
                prompts: false,
                turns: false,
                services: Vec::new(),
                descriptors: Vec::new(),
                rpcs: Vec::new(),
                config_type: String::new(),
            },
            tools: ToolCatalog::build()?,
            warm_up: Mutex::new(None),
            announcing: Mutex::new(None),
            host: Arc::new(Mutex::new(None)),
            services: None,
        })
    }

    /// The document kinds of the running modules, read through the
    /// module's handle once it starts.
    pub fn kinds(&self) -> Arc<dyn DocumentKinds> {
        Arc::new(HostKinds {
            host: Arc::clone(&self.host),
        })
    }

    /// The module serving `service`, its `pcx.v1` services over the same
    /// memory, to the other modules (the composition builds it over
    /// [`Self::kinds`]).
    #[must_use]
    pub fn serving(mut self, service: GrpcChannel) -> Self {
        self.manifest.services = vec![SERVICE.to_string(), BLOBS.to_string()];
        self.manifest.descriptors = post_cortex_proto::PCX_DESCRIPTOR_SET.to_vec();
        // Its events' message is in the descriptors it serves.
        self.manifest.emits = vec![EmittedEvent {
            name: RECORD_ADDED.to_string(),
            r#type: RECORD_ADDED_TYPE.to_string(),
            description: "A record was written, once per workspace of its session.".to_string(),
        }];
        self.manifest.rpcs = RPCS
            .iter()
            .map(|(service, method, tool, effect)| Rpc {
                method: format!("/{service}/{method}"),
                tool: (*tool).to_string(),
                visibility: if tool.is_empty() {
                    Vec::new()
                } else {
                    vec![ToolVisibility::Model.into()]
                },
                effect: (*effect).into(),
            })
            .collect();
        self.services = Some(service);
        self
    }
}

/// The document kinds adapter: the kinds the running modules' manifests
/// declare, read through memory's handle on its host.
struct HostKinds {
    /// None before the module starts.
    host: Handle,
}

#[async_trait]
impl DocumentKinds for HostKinds {
    async fn kinds(&self) -> Result<Vec<KindDeclaration>, KindsError> {
        let host = self.host.lock().clone();
        let host = host.ok_or_else(|| {
            KindsError::NotReady(
                "memory has no host to read the document kinds from yet".to_string(),
            )
        })?;
        let manifests = host.manifests().await.map_err(|e| {
            let reason = format!("the running modules' kinds: {}", e.message);
            match e.kind {
                ModuleErrorKind::FailedPrecondition => KindsError::NotReady(reason),
                _ => KindsError::Unavailable(reason),
            }
        })?;
        let mut kinds = Vec::new();
        for manifest in &manifests {
            for declared in &manifest.document_kinds {
                let kind = DocumentKind::new(&manifest.id, &declared.name)
                    .map_err(|e| KindsError::Invalid(e.to_string()))?;
                kinds.push(KindDeclaration {
                    kind,
                    type_name: declared.r#type.clone(),
                    title_field: declared.title_field.clone(),
                    searchable: declared.searchable != Some(false),
                    descriptors: manifest.descriptors.clone(),
                });
            }
        }
        Ok(kinds)
    }
}

#[async_trait]
impl Module for MemoryModule {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// Memory is opened by the composition that builds the module; the
    /// start loads the embedding model and the stored vectors and embeds
    /// the records stored without vectors, in the background, so no
    /// request starts that work (a search that comes first waits for the
    /// load, never for the embedding of missing records).
    async fn start(&self, ctx: HostContext) -> Result<(), ModuleError> {
        if !self.manifest.emits.is_empty() {
            let stop = CancellationToken::new();
            if let Some(before) = self.announcing.lock().replace(stop.clone()) {
                before.cancel();
            }
            tokio::spawn(announce::announce_records(
                Arc::clone(&ctx.host),
                Arc::clone(&self.memory),
                stop,
            ));
        }
        *self.host.lock() = Some(ctx.host);
        let mut warm_up = self.warm_up.lock();
        if warm_up
            .as_ref()
            .is_some_and(|(task, _)| !task.is_finished())
        {
            return Ok(());
        }
        let memory = Arc::clone(&self.memory);
        let stop = CancellationToken::new();
        let stopping = stop.clone();
        let task = tokio::spawn(async move {
            let started = std::time::Instant::now();
            match memory.admin.warm_up(&stopping).await {
                Ok(()) => info!("Embeddings warmed up in {:?}", started.elapsed()),
                Err(e) => error!("Embedding warm-up failed (search will retry): {e}"),
            }
        });
        *warm_up = Some((task, stop));
        Ok(())
    }

    fn mcp_tools(&self) -> Vec<ToolSpec> {
        self.tools.tools().to_vec()
    }

    /// Run a memory MCP tool; the scope's author is the default author of
    /// its writes.
    async fn call_mcp(
        &self,
        scope: &Scope,
        tool: &str,
        input: Value,
    ) -> Result<ToolOutput, ModuleError> {
        let kinds = HostKinds {
            host: Arc::clone(&self.host),
        };
        let person = post_cortex_core::core::card::person(scope.person.as_deref())
            .map_err(|e| ModuleError::new(ModuleErrorKind::InvalidArgument, e.to_string()))?;
        // A call that carries a reach is held to it (docs/connect.md 4a).
        let reach = scope
            .reach
            .as_deref()
            .map(reach::workspaces)
            .transpose()
            .map_err(refused)?;
        // Read as the operation table sees it: a call that would change
        // an archived workspace is refused (crate::archive). One without a
        // reach that does not read as its operation goes on to its
        // handler, which refuses it in its own words.
        match (classify::mcp(tool, &input), &reach) {
            (Err(_), None) => {}
            (Err(e), Some(_)) => {
                return Err(ModuleError::new(
                    ModuleErrorKind::InvalidArgument,
                    e.to_string(),
                ));
            }
            (Ok(call), reach) => {
                self.memory.open_to(&call).map_err(|e| {
                    ModuleError::new(ModuleErrorKind::FailedPrecondition, e.to_string())
                })?;
                if let Some(reach) = reach {
                    self.memory
                        .within_reach(&call, reach, person.is_some())
                        .await
                        .map_err(refused)?;
                }
            }
        }
        let caller = Caller {
            author: scope.author.clone(),
            module: scope.caller_module.as_deref(),
            kinds: &kinds,
            reach,
            person,
        };
        match dispatch(&self.memory, tool, input, &caller).await {
            Some(outcome) => outcome.map_err(ModuleError::from),
            None => Err(ModuleError::new(
                ModuleErrorKind::NotFound,
                format!("no memory tool {tool}"),
            )),
        }
    }

    /// Session start: the cards; a file edit: the file's memory; turn end:
    /// the captured turns.
    async fn on_event(&self, scope: &Scope, event: &Event) -> Result<Outcome, ModuleError> {
        events::answer(&self.memory, scope, event)
            .await
            .map_err(ModuleError::from)
    }

    /// Memory has no views of its own (its screens are the app's).
    fn services(&self) -> Option<GrpcChannel> {
        self.services.clone()
    }

    async fn resource(&self, uri: &str) -> Result<Resource, ModuleError> {
        Err(ModuleError::new(
            ModuleErrorKind::NotFound,
            format!("memory serves no resource {uri}"),
        ))
    }

    /// Tell the warm-up to stop and wait for it to finish the session it
    /// is on (no query or commit is cut off; it is aborted only past
    /// [`WARM_UP_DEADLINE`]), then write every session before the process
    /// ends.
    async fn stop(&self) -> Result<(), ModuleError> {
        if let Some(announcing) = self.announcing.lock().take() {
            announcing.cancel();
        }
        let warm_up = self.warm_up.lock().take();
        if let Some((mut task, stop)) = warm_up {
            stop.cancel();
            if tokio::time::timeout(WARM_UP_DEADLINE, &mut task)
                .await
                .is_err()
            {
                warn!("Embedding warm-up did not stop within {WARM_UP_DEADLINE:?}; aborted");
                task.abort();
            }
        }
        match self.memory.admin.flush_all(FLUSH_DEADLINE).await {
            0 => Ok(()),
            unwritten => Err(ModuleError::new(
                ModuleErrorKind::Unavailable,
                format!("{unwritten} session(s) not written within {FLUSH_DEADLINE:?}"),
            )),
        }
    }
}

/// A call outside its reach, as a module error.
fn refused(refusal: ReachRefusal) -> ModuleError {
    let kind = match refusal {
        ReachRefusal::NotAWorkspace(_) => ModuleErrorKind::InvalidArgument,
        ReachRefusal::Everything { .. }
        | ReachRefusal::Workspace { .. }
        | ReachRefusal::Session { .. }
        | ReachRefusal::Project { .. } => ModuleErrorKind::Denied,
    };
    ModuleError::new(kind, refusal.to_string())
}

#[cfg(test)]
mod tests;
