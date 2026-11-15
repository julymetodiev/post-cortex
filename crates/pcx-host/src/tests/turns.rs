// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A module's turns: the agent's calls in a turn a module asked reach that
//! module's uses and grant, under a token the host alone sets and forgets
//! when the turn ends; a client's turn is the agent's own.

use super::*;

use parking_lot::Mutex as SyncMutex;
use pcx_module_api::ModuleHost;
use pcx_module_api::agent::{TurnEnd, TurnEvent, TurnRequest, TurnStop, Turns};
use pcx_module_api::grpc::{self, GrpcChannel};
use pcx_module_api::model::{StopReason, Usage};
use pcx_module_api::rpc_tool::{self, RpcTool};
use post_cortex_proto::module as wire;
use post_cortex_proto::module::hooks_server::{Hooks, HooksServer};
use post_cortex_proto::skills;
use post_cortex_proto::skills::skills_server::{Skills, SkillsServer};
use serde_json::json;
use tokio::sync::mpsc;

type Handle = Arc<SyncMutex<Option<(Arc<dyn ModuleHost>, GrpcChannel)>>>;

/// The RPC `tool` serves, as its one tool.
const DISPATCH: &str = "/pcx.module.v1.Hooks/Dispatch";

/// The RPC `other` serves, as its one tool.
const LIST_SKILLS: &str = "/pcx.skills.v1.Skills/ListSkills";

/// The agent's turn: the tools it is listed, then a call of each tool its
/// prompt names (comma separated, any running module's RPC tool, through
/// the agent's channel), answered as `tools a,b | calls a=ok`.
struct AgentTurns {
    host: Handle,
    /// The token of each turn it ran.
    tokens: Arc<SyncMutex<Vec<Option<String>>>>,
    /// The reach each of its calls arrived with, as [`Answers`] saw it
    /// (empty: none). It calls without its turn's reach, as an agent that
    /// forgot it would.
    reached: Arc<SyncMutex<Vec<String>>>,
}

#[async_trait]
impl Turns for AgentTurns {
    async fn run(
        &self,
        scope: &Scope,
        request: &TurnRequest,
        _events: &mpsc::Sender<TurnEvent>,
    ) -> Result<TurnEnd, ModuleError> {
        self.tokens.lock().push(scope.turn.clone());
        let (host, channel) = self.host.lock().clone().unwrap();
        let listed: Vec<String> = host
            .tools(scope)
            .await?
            .into_iter()
            .map(|t| t.tool.spec.name)
            .collect();
        let every: Vec<RpcTool> = host
            .manifests()
            .await?
            .iter()
            .flat_map(|m| rpc_tool::tools(m).unwrap())
            .collect();
        let mut calls = Vec::new();
        for name in request.prompt.split(',').filter(|t| !t.is_empty()) {
            let tool = every.iter().find(|t| t.spec.name == name).unwrap();
            let forgetful = Scope {
                reach: None,
                ..scope.clone()
            };
            let called = match tool.call(channel.clone(), &forgetful, json!({})).await {
                Ok(output) => {
                    let reach = output
                        .data
                        .as_ref()
                        .and_then(|data| data["outcome"]["reason"].as_str())
                        .unwrap_or_default();
                    self.reached.lock().push(reach.to_string());
                    "ok".to_string()
                }
                Err(e) => format!("{:?}", e.kind),
            };
            calls.push(format!("{name}={called}"));
        }
        Ok(TurnEnd {
            stop: TurnStop::Answered(StopReason::EndTurn),
            steps: 1,
            usage: Usage::default(),
            answer: format!("tools {} | calls {}", listed.join(","), calls.join(",")),
        })
    }
}

/// Answers every dispatch with the reach the call arrived with, as its
/// outcome's reason.
struct Answers;

#[tonic::async_trait]
impl Hooks for Answers {
    async fn dispatch(
        &self,
        request: tonic::Request<wire::DispatchRequest>,
    ) -> Result<tonic::Response<wire::DispatchResponse>, tonic::Status> {
        let reach = request
            .metadata()
            .get(pcx_module_api::metadata::REACH)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        Ok(tonic::Response::new(wire::DispatchResponse {
            outcome: Some(Outcome {
                reason: reach,
                ..Outcome::default()
            }),
            error: None,
        }))
    }
}

/// Answers a listing of no skills, and nothing else.
struct NoSkills;

#[tonic::async_trait]
impl Skills for NoSkills {
    async fn add_skill(
        &self,
        _: tonic::Request<skills::AddSkillRequest>,
    ) -> Result<tonic::Response<skills::SkillRecord>, tonic::Status> {
        Err(tonic::Status::unimplemented("AddSkill"))
    }

    async fn update_skill(
        &self,
        _: tonic::Request<skills::UpdateSkillRequest>,
    ) -> Result<tonic::Response<skills::SkillRecord>, tonic::Status> {
        Err(tonic::Status::unimplemented("UpdateSkill"))
    }

    async fn list_skills(
        &self,
        _: tonic::Request<skills::ListSkillsRequest>,
    ) -> Result<tonic::Response<skills::SkillList>, tonic::Status> {
        Ok(tonic::Response::new(skills::SkillList::default()))
    }

    async fn get_skill(
        &self,
        _: tonic::Request<skills::GetSkillRequest>,
    ) -> Result<tonic::Response<skills::SkillRecord>, tonic::Status> {
        Err(tonic::Status::unimplemented("GetSkill"))
    }
}

/// A tool module whose one RPC is tool `tool`, of `visibility`: `tool`
/// serves [`DISPATCH`], any other [`LIST_SKILLS`].
struct Server(Manifest);

fn server(id: &str, tool: &str, visibility: Vec<i32>) -> Arc<Server> {
    // The chief's descriptors import the module protocol's.
    let (service, method, descriptors) = if id == "tool" {
        (
            "pcx.module.v1.Hooks",
            DISPATCH,
            post_cortex_proto::CHIEF_DESCRIPTOR_SET,
        )
    } else {
        (
            "pcx.skills.v1.Skills",
            LIST_SKILLS,
            post_cortex_proto::SKILLS_DESCRIPTOR_SET,
        )
    };
    Arc::new(Server(Manifest {
        id: id.to_string(),
        version: "0.1.0".to_string(),
        api: Some(API_VERSION),
        kind: ModuleKind::Tool.into(),
        services: vec![service.to_string()],
        descriptors: descriptors.to_vec(),
        rpcs: vec![wire::Rpc {
            method: method.to_string(),
            tool: tool.to_string(),
            visibility,
            effect: post_cortex_proto::module::Effect::Read.into(),
        }],
        ..Manifest::default()
    }))
}

#[async_trait]
impl Module for Server {
    fn manifest(&self) -> &Manifest {
        &self.0
    }

    async fn start(&self, _ctx: HostContext) -> Result<(), ModuleError> {
        Ok(())
    }

    async fn on_event(&self, _: &Scope, _: &Event) -> Result<Outcome, ModuleError> {
        Ok(Outcome::default())
    }

    async fn resource(&self, uri: &str) -> Result<Resource, ModuleError> {
        Err(ModuleError::new(ModuleErrorKind::NotFound, uri))
    }

    async fn stop(&self) -> Result<(), ModuleError> {
        Ok(())
    }

    fn services(&self) -> Option<GrpcChannel> {
        let routes = if self.0.id == "tool" {
            tonic::service::Routes::new(HooksServer::new(Answers))
        } else {
            tonic::service::Routes::new(SkillsServer::new(NoSkills))
        };
        Some(grpc::routes(routes))
    }
}

/// A module that keeps its handle: the agent (with its turns) or a module
/// that asks them.
struct Holder {
    manifest: Manifest,
    host: Handle,
    turns: Option<Arc<AgentTurns>>,
}

/// The holder `id`; one of `config` takes the chief's settings.
fn holder(id: &str, kind: ModuleKind, uses: &[&str]) -> Arc<Holder> {
    holding(id, kind, uses, false)
}

fn holding(id: &str, kind: ModuleKind, uses: &[&str], config: bool) -> Arc<Holder> {
    let host = Handle::default();
    let turns = (kind == ModuleKind::Agent).then(|| {
        Arc::new(AgentTurns {
            host: Arc::clone(&host),
            tokens: Arc::default(),
            reached: Arc::default(),
        })
    });
    Arc::new(Holder {
        manifest: Manifest {
            id: id.to_string(),
            version: "0.1.0".to_string(),
            api: Some(API_VERSION),
            kind: kind.into(),
            uses: uses.iter().map(|u| (*u).to_string()).collect(),
            descriptors: if config {
                post_cortex_proto::CHIEF_DESCRIPTOR_SET.to_vec()
            } else {
                Vec::new()
            },
            config_type: if config {
                "pcx.chief.v1.ChiefConfig".to_string()
            } else {
                String::new()
            },
            ..Manifest::default()
        },
        host,
        turns,
    })
}

#[async_trait]
impl Module for Holder {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    async fn start(&self, ctx: HostContext) -> Result<(), ModuleError> {
        *self.host.lock() = Some((ctx.host, ctx.channel));
        Ok(())
    }

    async fn on_event(&self, _: &Scope, _: &Event) -> Result<Outcome, ModuleError> {
        Ok(Outcome::default())
    }

    async fn resource(&self, uri: &str) -> Result<Resource, ModuleError> {
        Err(ModuleError::new(ModuleErrorKind::NotFound, uri))
    }

    async fn stop(&self) -> Result<(), ModuleError> {
        Ok(())
    }

    fn turns(&self) -> Option<Arc<dyn Turns>> {
        self.turns.clone().map(|t| t as Arc<dyn Turns>)
    }
}

/// A workspace of the chief's settings.
const WORKSPACE: &str = "6f1c2a3e-0000-4000-8000-000000000001";

/// The agent (using `tool` and `other`), the chief (using the agent and
/// `tool`, its turns granted `grant`) and the two tool modules: `tool`'s
/// RPC is `tool_rows` (a person only), `other`'s `other_echo` (any caller).
async fn chief_host(grant: Value) -> Result<(Host, Arc<Holder>, Arc<Holder>), HostError> {
    let agent = holder("agent", ModuleKind::Agent, &["tool", "other"]);
    let boss = holding("chief", ModuleKind::Tool, &["agent", "tool"], true);
    let modules: Vec<Arc<dyn Module>> = vec![
        server("tool", "tool_rows", vec![wire::ToolVisibility::App.into()]),
        server("other", "other_echo", Vec::new()),
        agent.clone(),
        boss.clone(),
    ];
    let config = HashMap::from([(
        "chief".to_string(),
        json!({ "workspace_id": WORKSPACE, "grant": grant }),
    )]);
    let host = Host::start(modules, &config, &HashMap::new()).await?;
    Ok((host, agent, boss))
}

/// The handle of a started module.
fn handle(module: &Holder) -> Arc<dyn ModuleHost> {
    module.host.lock().clone().unwrap().0
}

async fn turn(turns: &dyn Turns, scope: &Scope, prompt: &str) -> String {
    let (tx, _rx) = mpsc::channel(4);
    turns
        .run(scope, &TurnRequest::new("s1", prompt), &tx)
        .await
        .unwrap()
        .answer
}

#[tokio::test]
async fn a_modules_turn_reaches_its_uses_and_the_person_tools_its_grant_names() {
    let (host, _, boss) = chief_host(json!({ "person_tools": ["tool_rows"] }))
        .await
        .unwrap();
    let turns = handle(&boss).turns().await.unwrap();
    let scope = scope(Duration::from_secs(5));

    let answer = turn(turns.as_ref(), &scope, "tool_rows,other_echo").await;
    assert_eq!(
        answer,
        "tools tool_rows | calls tool_rows=ok,other_echo=Denied"
    );

    // A client's turn is the agent's own: every module it uses, no tool
    // only a person calls.
    let own = host.turns().unwrap();
    let answer = turn(own.as_ref(), &scope, "tool_rows,other_echo").await;
    assert_eq!(
        answer,
        "tools other_echo | calls tool_rows=Denied,other_echo=ok"
    );

    // A client that may only read runs no turn: its tools may change things.
    let reader = Scope {
        read_only: true,
        ..scope.clone()
    };
    let (tx, _rx) = mpsc::channel(4);
    let refused = own
        .run(&reader, &TurnRequest::new("s1", ""), &tx)
        .await
        .unwrap_err();
    assert_eq!(refused.kind, ModuleErrorKind::Denied);
}

#[tokio::test]
async fn a_withheld_tool_is_neither_listed_nor_called() {
    let (_host, _, boss) = chief_host(json!({
        "person_tools": ["tool_rows"],
        "withheld": ["tool_rows"],
    }))
    .await
    .unwrap();
    let turns = handle(&boss).turns().await.unwrap();
    let answer = turn(turns.as_ref(), &scope(Duration::from_secs(5)), "tool_rows").await;
    assert_eq!(answer, "tools  | calls tool_rows=Denied");
}

#[tokio::test]
async fn the_host_sets_the_token_and_forgets_it_when_the_turn_ends() {
    let (_host, agent, boss) = chief_host(json!({})).await.unwrap();
    let turns = handle(&boss).turns().await.unwrap();
    let forged = Scope {
        turn: Some("forged".to_string()),
        ..scope(Duration::from_secs(5))
    };
    turn(turns.as_ref(), &forged, "").await;
    let token = agent.turns.as_ref().unwrap().tokens.lock()[0].clone();
    let token = token.unwrap();
    assert_ne!(token, "forged");

    // The agent's tool listing in a turn that ended, or one that never ran,
    // is refused.
    let agent_host = handle(&agent);
    for token in [token, "forged".to_string()] {
        let stale = Scope {
            turn: Some(token),
            ..scope(Duration::from_secs(5))
        };
        let refused = agent_host.tools(&stale).await.unwrap_err();
        assert_eq!(refused.kind, ModuleErrorKind::Denied, "{refused:?}");
    }
}

/// The host holds the agent's calls in a turn to the workspaces the turn
/// was asked with, whatever the agent's call carried (docs/connect.md
/// section 4a, a member's reach).
#[tokio::test]
async fn the_agents_calls_in_a_turn_reach_no_more_than_the_turn_was_asked_with() {
    let (_host, agent, boss) = chief_host(json!({ "person_tools": ["tool_rows"] }))
        .await
        .unwrap();
    let turns = handle(&boss).turns().await.unwrap();
    let asked_with = |reach: Option<&[&str]>| Scope {
        reach: reach.map(|list| list.iter().map(|w| (*w).to_string()).collect()),
        ..scope(Duration::from_secs(5))
    };
    for reach in [Some(&["w1", "w2"][..]), Some(&[]), None] {
        let answer = turn(turns.as_ref(), &asked_with(reach), "tool_rows").await;
        assert_eq!(answer, "tools tool_rows | calls tool_rows=ok");
    }
    let reached = agent.turns.as_ref().unwrap().reached.lock().clone();
    // A turn asked with every workspace leaves the call as it came.
    assert_eq!(reached, ["w1,w2", "none", ""]);
}

#[tokio::test]
async fn a_grant_that_is_not_a_list_of_tool_names_is_refused_at_load() {
    let refused = chief_host(json!({ "person_tools": "tool_rows" })).await;
    let Err(HostError::InvalidConfig { module, .. }) = refused else {
        panic!("a malformed grant loaded");
    };
    assert_eq!(module, "chief");
}

mod own_name;
