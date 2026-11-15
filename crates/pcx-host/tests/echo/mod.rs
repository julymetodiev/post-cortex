// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The echo module the process test serves from its own binary.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use pcx_module_api::grpc::{self, GrpcChannel};
use pcx_module_api::view::{Body, HTML_MIME, View};
use pcx_module_api::{
    API_VERSION, Capability, Event, EventKind, FailurePolicy, HookSpecificOutput, HostContext,
    Manifest, Module, ModuleError, ModuleErrorKind, ModuleHost, ModuleKind, Outcome, Prompt,
    PromptArgument, PromptContent, PromptMessage, PromptRole, Resource, Scope, Subscription,
};
use post_cortex_proto::module as wire;
use post_cortex_proto::module::hooks_server::{Hooks, HooksServer};
use post_cortex_proto::skills;
use post_cortex_proto::skills::skills_client::SkillsClient;

/// The echo module's HTML view.
const CARD: &str = "ui://echo/card.html";

/// Its one RPC, its tool `echo`.
pub const DISPATCH: &str = "/pcx.module.v1.Hooks/Dispatch";

/// The RPC of the module it uses (`helper`).
pub const LIST_SKILLS: &str = "/pcx.skills.v1.Skills/ListSkills";

/// What a started echo holds: its handle and its channel to the host.
type Started = Arc<Mutex<Option<(Arc<dyn ModuleHost>, GrpcChannel)>>>;

/// Answers PreToolUse with its configured greeting, and its tool `echo`
/// (its RPC) by the event's prompt.
pub struct Echo {
    manifest: Manifest,
    greeting: Mutex<String>,
    started: Started,
}

/// Its RPC: for the event's prompt `echo` who called it, `group` its pid
/// and process group, `modules` the running modules, `relay` a call of the
/// helper's RPC through its channel; the answer is the outcome's decision.
struct Tool(Started);

#[tonic::async_trait]
impl Hooks for Tool {
    async fn dispatch(
        &self,
        request: tonic::Request<wire::DispatchRequest>,
    ) -> Result<tonic::Response<wire::DispatchResponse>, tonic::Status> {
        let scope = Scope::of(&request)?;
        let prompt = request
            .into_inner()
            .event
            .map(|e| e.prompt)
            .unwrap_or_default();
        let (host, channel) = self
            .0
            .lock()
            .clone()
            .ok_or_else(|| tonic::Status::failed_precondition("not started"))?;
        let decision = match prompt.as_str() {
            "echo" => format!("for {}", scope.author.clone().unwrap_or_default()),
            // A line on its stderr, which the host keeps in its log.
            "say" => {
                eprintln!("said hello");
                "said".to_string()
            }
            "group" => {
                let pid = std::process::id();
                format!("{pid} {}", process_group(pid))
            }
            "modules" => {
                let ids: Vec<String> = host
                    .manifests()
                    .await
                    .map_err(tonic::Status::from)?
                    .into_iter()
                    .map(|m| m.id)
                    .collect();
                ids.join(",")
            }
            "relay" => {
                let mut call = tonic::Request::new(skills::ListSkillsRequest::default());
                scope
                    .to_metadata(call.metadata_mut())
                    .map_err(tonic::Status::from)?;
                SkillsClient::new(channel).list_skills(call).await?;
                "relayed".to_string()
            }
            other => return Err(tonic::Status::not_found(format!("no tool {other}"))),
        };
        Ok(tonic::Response::new(wire::DispatchResponse {
            outcome: Some(Outcome {
                decision,
                ..Outcome::default()
            }),
            error: None,
        }))
    }
}

impl Echo {
    pub fn new() -> Self {
        Self {
            manifest: Manifest {
                emits: Vec::new(),
                follows: Vec::new(),
                workflow_templates: Vec::new(),
                id: "echo".to_string(),
                version: "1.0.0".to_string(),
                api: Some(API_VERSION),
                kind: ModuleKind::Tool.into(),
                events: vec![Subscription {
                    kind: EventKind::PreTool.into(),
                    deadline_ms: 2_000,
                    policy: FailurePolicy::FailOpen.into(),
                    memory: None,
                }],
                capabilities: vec![
                    Capability::ReadConversation.into(),
                    Capability::UiHtml.into(),
                ],
                filesystem: Vec::new(),
                document_kinds: Vec::new(),
                uses: vec!["helper".to_string()],
                prompts: true,
                turns: false,
                services: vec!["pcx.module.v1.Hooks".to_string()],
                // The chief's descriptors hold the module protocol's; the
                // greet module's its settings.
                descriptors: descriptors(),
                rpcs: vec![wire::Rpc {
                    method: DISPATCH.to_string(),
                    tool: "echo".to_string(),
                    visibility: Vec::new(),
                    effect: post_cortex_proto::module::Effect::Read.into(),
                }],
                config_type: CONFIG.to_string(),
                views: vec![View {
                    id: "card".to_string(),
                    title: "Card".to_string(),
                    description: String::new(),
                    body: Some(Body::ResourceUri(CARD.to_string())),
                }],
                description: String::new(),
            },
            greeting: Mutex::new(String::new()),
            started: Arc::default(),
        }
    }
}

#[async_trait]
impl Module for Echo {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    async fn start(&self, ctx: HostContext) -> Result<(), ModuleError> {
        let config: pcx_greet::pb::GreetConfig = ctx.config()?;
        *self.greeting.lock() = config.greeting;
        *self.started.lock() = Some((ctx.host, ctx.channel));
        Ok(())
    }

    async fn on_event(&self, _scope: &Scope, event: &Event) -> Result<Outcome, ModuleError> {
        Ok(Outcome {
            hook_specific_output: Some(HookSpecificOutput {
                hook_event_name: event.hook_event_name.clone(),
                additional_context: format!("{} {}", self.greeting.lock(), event.tool_name),
            }),
            ..Outcome::default()
        })
    }

    async fn resource(&self, uri: &str) -> Result<Resource, ModuleError> {
        if uri != CARD {
            return Err(ModuleError::new(ModuleErrorKind::NotFound, uri));
        }
        Ok(Resource {
            uri: CARD.to_string(),
            mime_type: HTML_MIME.to_string(),
            text: format!("<p>{}</p>", self.greeting.lock()),
        })
    }

    async fn prompts(&self, _scope: &Scope) -> Result<Vec<Prompt>, ModuleError> {
        Ok(vec![Prompt {
            name: "greeting".to_string(),
            description: "Greet someone".to_string(),
            arguments: vec![PromptArgument {
                name: "name".to_string(),
                description: "Whom".to_string(),
                required: true,
            }],
        }])
    }

    async fn prompt(
        &self,
        scope: &Scope,
        name: &str,
        arguments: &HashMap<String, String>,
    ) -> Result<PromptContent, ModuleError> {
        if name != "greeting" {
            return Err(ModuleError::new(ModuleErrorKind::NotFound, name));
        }
        Ok(PromptContent {
            description: "Greet someone".to_string(),
            messages: vec![PromptMessage {
                role: PromptRole::User.into(),
                text: format!(
                    "{} {}, from {}",
                    self.greeting.lock(),
                    arguments["name"],
                    scope.author.clone().unwrap_or_default()
                ),
            }],
        })
    }

    async fn stop(&self) -> Result<(), ModuleError> {
        Ok(())
    }

    fn services(&self) -> Option<GrpcChannel> {
        let tool = Tool(Arc::clone(&self.started));
        Some(grpc::routes(tonic::service::Routes::new(HooksServer::new(
            tool,
        ))))
    }
}

/// The chief's descriptors (with the module protocol's) and the greet
/// module's (its settings), as one set; read as they are, so the options
/// keep their extensions.
fn descriptors() -> Vec<u8> {
    let mut pool =
        prost_reflect::DescriptorPool::decode(post_cortex_proto::CHIEF_DESCRIPTOR_SET).unwrap();
    pool.decode_file_descriptor_set(pcx_greet::DESCRIPTORS)
        .unwrap();
    pool.encode_to_vec()
}

/// The message of the echo module's configuration: the greet module's,
/// its greeting.
pub const CONFIG: &str = "example.greet.v1.GreetConfig";

/// The process group of `pid`, as `ps` reports it.
pub fn process_group(pid: u32) -> u32 {
    let out = std::process::Command::new("ps")
        .args(["-o", "pgid=", "-p", &pid.to_string()])
        .output()
        .unwrap();
    String::from_utf8(out.stdout)
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}
