// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The skills library over memory as the daemon composes it (ARCHITECTURE
//! section 8): its skills kept as documents in its configured workspace,
//! and each one a prompt of the host.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use pcx_host::Host;
use pcx_module_api::{Module, ModuleErrorKind, Scope};
use pcx_skills::SkillsModule;
use post_cortex_memory::capabilities::CreateWorkspace;
use serde_json::json;
use tokio_util::sync::CancellationToken;

fn scope() -> Scope {
    Scope {
        tenant: String::new(),
        profile: String::new(),
        session_id: None,
        workspace_id: None,
        author: Some("claude-code".to_string()),
        caller_module: None,
        call_depth: 0,
        turn: None,
        reach: None,
        role: None,
        read_only: false,
        person: None,
        deadline: Instant::now() + Duration::from_secs(10),
        cancel: CancellationToken::new(),
    }
}

#[tokio::test]
async fn a_skill_is_a_document_of_the_library_and_a_prompt_of_the_host() {
    let data = tempfile::tempdir().unwrap();
    let memory = Arc::new(
        crate::memory_tests::rocksdb_memory(
            &data,
            post_cortex_memory::SystemConfig {
                enable_embeddings: false,
                ..Default::default()
            },
        )
        .await
        .unwrap(),
    );
    let ws = memory
        .workspaces
        .create(CreateWorkspace {
            name: "library".into(),
            description: String::new(),
        })
        .await
        .unwrap()
        .id;
    let modules: Vec<Arc<dyn Module>> = vec![
        crate::memory_tests::memory_module(&memory).0,
        Arc::new(SkillsModule::new()),
    ];
    let config = HashMap::from([(
        SkillsModule::ID.to_string(),
        json!({ "workspace_id": ws.to_string() }),
    )]);
    let host = Host::start(modules, &config, &HashMap::new())
        .await
        .unwrap();

    let added = host
        .call_for_agent(
            &scope(),
            "skill_add",
            json!({
                "name": "review",
                "description": "Review a change",
                "instructions": "Read the diff, then run the tests.",
                "tools": ["bash"]
            }),
        )
        .await
        .unwrap();
    assert_eq!(added.data.unwrap()["author"], json!("claude-code"));

    // A document of the library's workspace, of the skill's kind.
    let kept = host
        .call_for_agent(
            &scope(),
            "manage_document",
            json!({ "action": "list", "workspace_id": ws.to_string(), "kind": "skills/skill" }),
        )
        .await
        .unwrap()
        .data
        .unwrap();
    // The library's built-in skills are added at start, maybe already.
    let builtin = |name: &str| name == "workflow-builder" || name == "module-builder";
    let documents = kept["documents"].as_array().unwrap();
    let mine = documents
        .iter()
        .filter(|d| !builtin(d["title"].as_str().unwrap_or_default()))
        .count();
    assert_eq!(mine, 1, "{documents:?}");

    let prompts = host.prompts(&scope()).await;
    let listed: Vec<(&str, &str)> = prompts
        .iter()
        .filter(|p| !builtin(&p.name))
        .map(|p| (p.name.as_str(), p.description.as_str()))
        .collect();
    assert_eq!(listed, [("review", "Review a change")]);
    let filled = host
        .prompt(&scope(), "review", &HashMap::new())
        .await
        .unwrap();
    assert_eq!(
        filled.messages[0].text,
        "Read the diff, then run the tests.\n\nTools this skill needs: bash."
    );
    let with_arguments = host
        .prompt(
            &scope(),
            "review",
            &HashMap::from([("file".to_string(), "a.rs".to_string())]),
        )
        .await
        .unwrap_err();
    assert_eq!(with_arguments.kind, ModuleErrorKind::InvalidArgument);

    // Without its workspace the library does not start.
    let unconfigured: Vec<Arc<dyn Module>> = vec![
        crate::memory_tests::memory_module(&memory).0,
        Arc::new(SkillsModule::new()),
    ];
    assert!(
        Host::start(unconfigured, &HashMap::new(), &HashMap::new())
            .await
            .is_err()
    );
}

/// A client calls a module's RPC on the instance's gRPC port as any gRPC
/// call (W23, what a view does): behind the instance's token, routed by
/// path to the module that serves it; a path no module serves is not
/// found.
#[tokio::test]
async fn a_client_calls_a_modules_rpc_through_the_instance() {
    use post_cortex_proto::chief::GetRightsRequest;
    use post_cortex_proto::chief::chief_client::ChiefClient;
    use post_cortex_proto::skills::skills_client::SkillsClient;
    use post_cortex_proto::skills::{AddSkillRequest, ListSkillsRequest};

    let data = tempfile::tempdir().unwrap();
    let memory = Arc::new(
        crate::memory_tests::rocksdb_memory(
            &data,
            post_cortex_memory::SystemConfig {
                enable_embeddings: false,
                ..Default::default()
            },
        )
        .await
        .unwrap(),
    );
    let ws = memory
        .workspaces
        .create(CreateWorkspace {
            name: "library".into(),
            description: String::new(),
        })
        .await
        .unwrap()
        .id;
    let (memory_module, kinds) = crate::memory_tests::memory_module(&memory);
    let modules: Vec<Arc<dyn Module>> = vec![memory_module, Arc::new(SkillsModule::new())];
    let config = HashMap::from([(
        SkillsModule::ID.to_string(),
        json!({ "workspace_id": ws.to_string() }),
    )]);
    let host = Arc::new(
        Host::start(modules, &config, &HashMap::new())
            .await
            .unwrap(),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let routes = crate::compose::grpc_routes(memory, kinds, host, None, Some("s3cret".into()));
    let served = tokio::spawn(
        tonic::transport::Server::builder()
            .add_routes(routes)
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener)),
    );
    let channel = tonic::transport::Channel::from_shared(format!("http://{addr}"))
        .unwrap()
        .connect()
        .await
        .unwrap();
    let add = AddSkillRequest {
        name: "review".into(),
        description: "Review a change".into(),
        instructions: "Read the diff.".into(),
        tools: vec!["bash".into()],
    };

    let mut anonymous = SkillsClient::new(channel.clone());
    let refused = anonymous.add_skill(add.clone()).await.unwrap_err();
    assert_eq!(refused.code(), tonic::Code::Unauthenticated);

    let bearer = |mut request: tonic::Request<()>| {
        request
            .metadata_mut()
            .insert("authorization", "Bearer s3cret".parse().unwrap());
        Ok::<_, tonic::Status>(request)
    };
    let mut client = SkillsClient::with_interceptor(channel.clone(), bearer);
    let added = client.add_skill(add).await.unwrap().into_inner();
    assert_eq!(added.skill.unwrap().name, "review");
    let listed = client
        .list_skills(ListSkillsRequest {})
        .await
        .unwrap()
        .into_inner();
    let mine: Vec<&str> = listed
        .skills
        .iter()
        .filter_map(|s| s.skill.as_ref().map(|k| k.name.as_str()))
        .filter(|n| *n != "workflow-builder" && *n != "module-builder")
        .collect();
    assert_eq!(mine, ["review"], "the built-in skills aside");

    // A service no running module serves.
    let mut chief = ChiefClient::with_interceptor(channel, bearer);
    let missing = chief.get_rights(GetRightsRequest {}).await.unwrap_err();
    assert_eq!(missing.code(), tonic::Code::NotFound, "{missing:?}");
    served.abort();
}
