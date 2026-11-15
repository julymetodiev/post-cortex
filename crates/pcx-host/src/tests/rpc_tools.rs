// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A module's RPCs as tools (W23): listed with their comment and the
//! schema of their request from the module's descriptors, called with
//! JSON read into the request message and answered as the JSON of the
//! response, a person-only one refused to an agent.

use super::*;

use post_cortex_proto::module::{Rpc, ToolVisibility};
use post_cortex_proto::skills::skills_server::{Skills, SkillsServer};
use post_cortex_proto::skills::{
    AddSkillRequest, GetSkillRequest, ListSkillsRequest, Skill, SkillList, SkillRecord,
    UpdateSkillRequest,
};
use serde_json::json;
use tonic::{Request, Response, Status};

/// A library that has every skill asked for, named by its description.
struct Library;

#[tonic::async_trait]
impl Skills for Library {
    async fn add_skill(
        &self,
        _: Request<AddSkillRequest>,
    ) -> Result<Response<SkillRecord>, Status> {
        Err(Status::unimplemented("not in this test"))
    }

    async fn update_skill(
        &self,
        _: Request<UpdateSkillRequest>,
    ) -> Result<Response<SkillRecord>, Status> {
        Err(Status::unimplemented("not in this test"))
    }

    async fn list_skills(
        &self,
        _: Request<ListSkillsRequest>,
    ) -> Result<Response<SkillList>, Status> {
        Ok(Response::new(SkillList::default()))
    }

    async fn get_skill(
        &self,
        request: Request<GetSkillRequest>,
    ) -> Result<Response<SkillRecord>, Status> {
        let name = request.into_inner().name;
        Ok(Response::new(SkillRecord {
            id: "s1".to_string(),
            version: 3,
            skill: Some(Skill {
                description: format!("all about {name}"),
                name,
                tools: vec!["bash".to_string()],
            }),
            instructions: "Do it.".to_string(),
            ..SkillRecord::default()
        }))
    }
}

/// A module serving the library, `skill_get` for both, `skill_list` for a
/// person only.
struct Served {
    manifest: Manifest,
    host: parking_lot::Mutex<Option<Arc<dyn pcx_module_api::ModuleHost>>>,
}

fn served() -> Served {
    let rpc = |method: &str, tool: &str, visibility: Vec<i32>| Rpc {
        method: format!("/pcx.skills.v1.Skills/{method}"),
        tool: tool.to_string(),
        visibility,
        effect: post_cortex_proto::module::Effect::Read.into(),
    };
    Served {
        manifest: Manifest {
            id: "library".to_string(),
            version: "0.1.0".to_string(),
            api: Some(API_VERSION),
            kind: ModuleKind::Tool.into(),
            services: vec!["pcx.skills.v1.Skills".to_string()],
            descriptors: post_cortex_proto::SKILLS_DESCRIPTOR_SET.to_vec(),
            rpcs: vec![
                rpc("GetSkill", "skill_get", Vec::new()),
                rpc("ListSkills", "skill_list", vec![ToolVisibility::App.into()]),
            ],
            ..Manifest::default()
        },
        host: parking_lot::Mutex::new(None),
    }
}

/// A module of `id`, using `uses`, that serves nothing but keeps its handle.
fn caller(id: &str, uses: &[&str]) -> Arc<Served> {
    Arc::new(Served {
        manifest: Manifest {
            id: id.to_string(),
            version: "0.1.0".to_string(),
            api: Some(API_VERSION),
            kind: ModuleKind::Tool.into(),
            uses: uses.iter().map(|u| (*u).to_string()).collect(),
            ..Manifest::default()
        },
        host: parking_lot::Mutex::new(None),
    })
}

/// What `module` reaches, by tool.
async fn reached(module: &Served) -> Vec<String> {
    let handle = module.host.lock().clone().unwrap();
    handle
        .tools(&scope(Duration::from_secs(1)))
        .await
        .unwrap()
        .into_iter()
        .map(|used| used.tool.spec.name)
        .collect()
}

#[async_trait]
impl Module for Served {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    async fn start(&self, ctx: HostContext) -> Result<(), ModuleError> {
        *self.host.lock() = Some(ctx.host);
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

    fn services(&self) -> Option<pcx_module_api::grpc::GrpcChannel> {
        if self.manifest.services.is_empty() {
            return None;
        }
        Some(pcx_module_api::grpc::routes(tonic::service::Routes::new(
            SkillsServer::new(Library),
        )))
    }
}

#[tokio::test]
async fn an_rpc_is_a_tool_of_its_descriptors_called_with_json() {
    let host = Host::start(vec![Arc::new(served())], &HashMap::new(), &HashMap::new())
        .await
        .unwrap();
    let tools = host.tools();
    let get = tools.iter().find(|t| t.name == "skill_get").unwrap();
    assert_eq!(get.description, "One skill with its instructions.");
    assert_eq!(get.effect, pcx_module_api::Effect::Read);
    assert_eq!(get.input_schema["properties"]["name"]["type"], "string");
    assert_eq!(
        get.input_schema["properties"]["name"]["description"],
        "The skill's name."
    );
    // A person-only RPC is no agent's tool.
    assert!(tools.iter().all(|t| t.name != "skill_list"));

    let within = scope(Duration::from_secs(5));
    let answer = host
        .call_for_agent(&within, "skill_get", json!({ "name": "review" }))
        .await
        .unwrap();
    let data = answer.data.unwrap();
    assert_eq!(data["skill"]["description"], "all about review");
    assert_eq!(data["skill"]["tools"], json!(["bash"]));
    assert_eq!(data["version"], "3");

    let wrong = host
        .call_for_agent(&within, "skill_get", json!({ "nme": "review" }))
        .await
        .unwrap_err();
    assert_eq!(wrong.kind, ModuleErrorKind::InvalidArgument);
    let refused = host
        .call_for_agent(&within, "skill_list", json!({}))
        .await
        .unwrap_err();
    assert_eq!(refused.kind, ModuleErrorKind::Denied);
}

#[tokio::test]
async fn an_rpc_without_an_effect_refuses_its_manifest() {
    for tool in ["skill_get", ""] {
        let mut module = served();
        module.manifest.rpcs.push(Rpc {
            method: "/pcx.skills.v1.Skills/AddSkill".to_string(),
            tool: tool.replace("skill_get", "skill_add"),
            visibility: Vec::new(),
            effect: post_cortex_proto::module::Effect::Unspecified.into(),
        });
        let refused = Host::start(vec![Arc::new(module)], &HashMap::new(), &HashMap::new()).await;
        assert!(
            matches!(refused, Err(HostError::InvalidManifest { .. })),
            "tool {tool:?}"
        );
    }
}

#[tokio::test]
async fn naming_dynamic_reaches_the_modules_added_in_place_and_no_others() {
    let (names, stranger) = (caller("names", &["dynamic"]), caller("stranger", &[]));
    let started: Vec<Arc<dyn Module>> = vec![names.clone(), stranger.clone()];

    // A module started with the host is not reached by naming `dynamic`.
    let mut with_library = started.clone();
    with_library.push(Arc::new(served()));
    let host = Host::start(with_library, &HashMap::new(), &HashMap::new())
        .await
        .unwrap();
    assert!(reached(&names).await.is_empty());
    host.stop().await;

    // One added in place is, while it runs, and only by who names it.
    let (names, stranger) = (caller("names", &["dynamic"]), caller("stranger", &[]));
    let started: Vec<Arc<dyn Module>> = vec![names.clone(), stranger.clone()];
    let host = Host::start(started, &HashMap::new(), &HashMap::new())
        .await
        .unwrap();
    assert!(reached(&names).await.is_empty());
    host.change(vec![Arc::new(served())], &[]).await.unwrap();
    assert_eq!(reached(&names).await, ["skill_get"]);
    assert!(reached(&stranger).await.is_empty());
    host.change(Vec::new(), &["library".to_string()])
        .await
        .unwrap();
    assert!(reached(&names).await.is_empty());
    host.stop().await;
}
