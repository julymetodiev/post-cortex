// Copyright (c) 2026 Julius ML
// MIT License

//! The same request through MCP, gRPC and REST returns one response shape.

use super::*;
use axum::body::Body;
use axum::http::Request;
use post_cortex_core::core::context_update::{
    EntityData, EntityRelationship, EntityType, RelationType, UpdateContent, UpdateType,
};
use post_cortex_core::services::UpdateContextRequest;
use post_cortex_proto::pb;
use post_cortex_proto::pb::post_cortex_server::PostCortex;
use tonic::Request as GrpcRequest;
use tower::ServiceExt;

/// A session with one decision about two entities, in a workspace.
async fn seeded(t: &Transports) -> (String, String) {
    let sid = t.memory.sessions.create(None, None).await.unwrap();
    let entity = |name: &str| EntityData {
        aliases: Vec::new(),
        name: name.into(),
        entity_type: EntityType::Concept,
        first_mentioned: chrono::Utc::now(),
        last_mentioned: chrono::Utc::now(),
        mention_count: 1,
        importance_score: 1.0,
        description: None,
    };
    t.memory
        .records
        .write(UpdateContextRequest {
            session_id: sid,
            interaction_type: UpdateType::DecisionMade,
            content: UpdateContent {
                title: "Use cards".into(),
                description: "always in front".into(),
                details: vec![],
                examples: vec![],
                implications: vec![],
            },
            entities: vec![entity("cards"), entity("sessions")],
            relations: vec![EntityRelationship {
                from_entity: "cards".into(),
                to_entity: "sessions".into(),
                relation_type: RelationType::DependsOn,
                context: "injected".into(),
            }],
            code_reference: None,
            author: None,
            derived_from: Vec::new(),
            meta: Default::default(),
        })
        .await
        .unwrap();
    let ws = t
        .memory
        .workspaces
        .create(post_cortex_memory::capabilities::CreateWorkspace {
            name: "ws".into(),
            description: String::new(),
        })
        .await
        .unwrap();
    t.memory
        .workspaces
        .add_session(
            ws.id,
            sid,
            post_cortex_core::workspace::SessionRole::Primary,
        )
        .await
        .unwrap();
    // The record's embedding is derived work: done before anyone searches
    // (the model loads within the wait: the 120 s every embedding test gives it).
    assert!(
        t.memory
            .records
            .wait_idle(std::time::Duration::from_secs(120))
            .await
    );
    (sid.to_string(), ws.id.to_string())
}

async fn mcp(t: &mut Transports, tool: &str, args: Value) -> Value {
    let message = t.mcp.call_raw(tool, args).await;
    mcp_data(&message).unwrap_or_else(|| panic!("{tool}: no data in {message}"))
}

/// The MCP tool and the typed gRPC message carry one assembly, over a
/// session and a workspace (its records come from search: embeddings on).
#[tokio::test]
async fn assembly_is_one_data() {
    let mut t = transports_with(true).await;
    let (sid, ws) = seeded(&t).await;
    for (session, workspace) in [(sid.clone(), None), (String::new(), Some(ws.clone()))] {
        let mut args = json!({"query": "cards", "select": "$"});
        match &workspace {
            Some(ws) => args["workspace_id"] = json!(ws),
            None => args["session_id"] = json!(session),
        }
        let over_mcp = mcp(&mut t, "assemble_context", args).await;
        let over_grpc = t
            .grpc
            .assemble_context(GrpcRequest::new(pb::AssembleContextRequest {
                session_id: session,
                workspace_id: workspace,
                query: "cards".into(),
                ..Default::default()
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(over_mcp["items"][0]["text"], "Use cards always in front");
        assert_eq!(over_grpc.items[0].text, "Use cards always in front");
        assert_eq!(over_mcp["total_tokens"], json!(over_grpc.total_tokens));
        // gRPC carries relation types by their wire names (they were Debug
        // names: `DependsOn`).
        let relation = &over_grpc.entity_context[0].relationships[0];
        assert_eq!(relation.relation_type, "depends_on");
    }
    t.ct.cancel();
}

/// The MCP tool and the typed gRPC message find the same hits (search
/// needs embeddings on).
#[tokio::test]
async fn search_is_one_data() {
    let mut t = transports_with(true).await;
    let (_, ws) = seeded(&t).await;
    let over_mcp = mcp(
        &mut t,
        "semantic_search",
        json!({"query": "cards", "scope": "workspace", "scope_id": ws, "select": "$"}),
    )
    .await;
    let over_grpc = t
        .grpc
        .semantic_search(GrpcRequest::new(pb::SemanticSearchRequest {
            query: "cards".into(),
            workspace_id: Some(ws.clone()),
            max_results: 10,
            ..Default::default()
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(over_mcp["workspace_id"], json!(ws));
    let ids: Vec<&str> = over_grpc
        .results
        .iter()
        .map(|r| r.entry_id.as_str())
        .collect();
    let mcp_ids: Vec<&str> = over_mcp["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["content_id"].as_str().unwrap())
        .collect();
    assert!(!ids.is_empty());
    assert_eq!(ids, mcp_ids);
    t.ct.cancel();
}

/// The views of `query_conversation_context` and `get_structured_summary`
/// carry the same data as the typed gRPC messages (W23).
#[tokio::test]
async fn views_carry_one_data() {
    let mut t = transports().await;
    let (sid, _) = seeded(&t).await;
    let query = |query_type: &str| json!({"session_id": sid, "query_type": query_type, "parameters": {"entity": "cards"}});
    let kinds = post_cortex_memory::MemoryModule::new(t.memory.clone())
        .unwrap()
        .kinds();
    let g = PcxGrpcService::new(t.memory.clone(), crate::compose::BUILD, kinds);
    let s = || sid.clone();

    let over_mcp = mcp(&mut t, "get_structured_summary", json!({"session_id": sid})).await;
    let summary = g
        .get_structured_summary(GrpcRequest::new(pb::GetStructuredSummaryRequest {
            session_id: s(),
            decisions_limit: Some(10),
            entities_limit: Some(20),
            questions_limit: Some(5),
            concepts_limit: Some(10),
            ..Default::default()
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        over_mcp["important_entities"],
        json!(summary.important_entities)
    );
    assert_eq!(
        over_mcp["key_decisions"].as_array().unwrap().len(),
        summary.key_decisions.len(),
        "summary"
    );
    assert_eq!(over_mcp["session_id"], json!(summary.session_id));

    let over_mcp = mcp(&mut t, "query_conversation_context", query("key_decisions")).await;
    let decisions = g
        .get_key_decisions(GrpcRequest::new(pb::GetKeyDecisionsRequest {
            session_id: s(),
        }))
        .await
        .unwrap()
        .into_inner()
        .decisions;
    let described: Vec<&str> = decisions.iter().map(|d| d.description.as_str()).collect();
    let over_mcp: Vec<&str> = over_mcp
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["description"].as_str().unwrap())
        .collect();
    assert_eq!(over_mcp, described, "key_decisions");

    let over_mcp = mcp(&mut t, "query_conversation_context", query("key_insights")).await;
    let insights = g
        .get_key_insights(GrpcRequest::new(pb::GetKeyInsightsRequest {
            session_id: s(),
            limit: None,
        }))
        .await
        .unwrap()
        .into_inner()
        .insights;
    assert_eq!(over_mcp, json!(insights), "key_insights");

    let over_mcp = mcp(
        &mut t,
        "query_conversation_context",
        query("entity_importance"),
    )
    .await;
    let importance = g
        .get_entity_importance(GrpcRequest::new(pb::GetEntityImportanceRequest {
            session_id: s(),
            ..Default::default()
        }))
        .await
        .unwrap()
        .into_inner();
    let names: Vec<&str> = importance
        .entities
        .iter()
        .map(|e| e.entity_name.as_str())
        .collect();
    let over_mcp_names: Vec<&str> = over_mcp["entities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["entity_name"].as_str().unwrap())
        .collect();
    assert_eq!(over_mcp_names, names, "entity_importance");
    assert_eq!(
        over_mcp["pagination"]["total_entities"],
        json!(importance.total_entities)
    );

    let over_mcp = mcp(
        &mut t,
        "query_conversation_context",
        query("entity_network"),
    )
    .await;
    let network = g
        .get_entity_network(GrpcRequest::new(pb::GetEntityNetworkRequest {
            session_id: s(),
            ..Default::default()
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(over_mcp["center_entity"], json!(network.center_entity));
    assert_eq!(
        over_mcp["relationships"].as_array().unwrap().len(),
        network.relationships.len(),
        "entity_network"
    );
    assert_eq!(network.relationships[0].relation_type, "depends_on");

    let over_mcp = mcp(
        &mut t,
        "query_conversation_context",
        query("session_statistics"),
    )
    .await;
    let stats = g
        .get_session_statistics(GrpcRequest::new(pb::GetSessionStatisticsRequest {
            session_id: s(),
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(over_mcp["total_updates"], json!(stats.total_updates));
    assert_eq!(over_mcp["entity_count"], json!(stats.entity_count));

    let over_mcp = mcp(&mut t, "query_conversation_context", query("entity")).await;
    let view = g
        .get_graph_view(GrpcRequest::new(pb::GetGraphViewRequest {
            session_id: s(),
            view: Some(pb::get_graph_view_request::View::Entity(
                pb::EntityViewParams {
                    entity: "cards".into(),
                    limit: 0,
                },
            )),
            ..Default::default()
        }))
        .await
        .unwrap()
        .into_inner();
    let Some(pb::graph_view::View::Entity(entity)) = view.view else {
        panic!("not the entity view: {view:?}");
    };
    assert_eq!(
        over_mcp["entity"]["name"],
        json!(entity.entity.unwrap().name)
    );
    assert_eq!(over_mcp["total_records"], json!(entity.total_records));
    t.ct.cancel();
}

async fn rest(t: &Transports, request: Request<Body>) -> Value {
    let response = t.router.clone().oneshot(request).await.unwrap();
    assert!(response.status().is_success(), "{}", response.status());
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn post(path: &str, body: Value) -> Request<Body> {
    Request::post(path)
        .header("host", "localhost:3737")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

/// Health and project resolution are one shape on MCP and REST.
#[tokio::test]
async fn health_and_projects_are_one_shape() {
    let mut t = transports().await;
    let over_mcp = mcp(&mut t, "admin", json!({"action": "health"})).await;
    let request = Request::get("/health").body(Body::empty()).unwrap();
    let mut over_rest = rest(&t, request).await;
    let map = over_rest.as_object_mut().unwrap();
    assert_eq!(map.remove("status"), Some(json!("ok")));
    assert_eq!(map.remove("service"), Some(json!("post-cortex")));
    // The host's module health is `/health`'s own (the MCP tool is
    // memory's).
    assert!(map.remove("modules").is_some_and(|m| m.is_array()));
    let keys = |v: &Value| {
        v.as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>()
    };
    assert_eq!(keys(&over_mcp), keys(&over_rest), "health");

    let body = json!({"key": "github.com/a/b", "name": "b"});
    let created = rest(&t, post("/api/projects/resolve", body.clone())).await;
    let mut args = body;
    args["action"] = json!("resolve_project");
    let again = mcp(&mut t, "session", args).await;
    assert_eq!(keys(&created), keys(&again));
    assert_eq!(created["session_id"], again["session_id"]);
    assert_eq!(again["created"], json!(false));
    t.ct.cancel();
}
