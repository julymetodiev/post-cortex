// Copyright (c) 2026 Julius ML
// MIT License

//! `semantic_search` shows a long result as its passage for the query
//! (`detail` snippet, the default), whole with `detail` full; a record a
//! snippet cut is read whole with `query_conversation_context` `record`.

use super::*;
use post_cortex_core::core::context_update::{
    EntityData, EntityRelationship, EntityType, RelationType, UpdateContent, UpdateType,
};
use post_cortex_core::services::UpdateContextRequest;

async fn call(t: &mut Transports, tool: &str, args: Value) -> Value {
    let message = t.mcp.call_raw(tool, args).await;
    mcp_data(&message).unwrap_or_else(|| panic!("{tool}: no data in {message}"))
}

async fn write(
    t: &Transports,
    sid: uuid::Uuid,
    title: &str,
    description: &str,
) -> post_cortex_core::services::UpdateContextResponse {
    t.memory
        .records
        .write(request(sid, title, description))
        .await
        .unwrap()
}

/// A decision about staging and pcx-swap, with one relation.
fn request(sid: uuid::Uuid, title: &str, description: &str) -> UpdateContextRequest {
    UpdateContextRequest {
        session_id: sid,
        interaction_type: UpdateType::DecisionMade,
        content: UpdateContent {
            title: title.into(),
            description: description.into(),
            details: vec![],
            examples: vec![],
            implications: vec![],
        },
        entities: ["pcx-swap", "staging"]
            .into_iter()
            .map(|name| EntityData {
                aliases: Vec::new(),
                name: name.into(),
                entity_type: EntityType::Technology,
                first_mentioned: chrono::Utc::now(),
                last_mentioned: chrono::Utc::now(),
                mention_count: 1,
                importance_score: 1.0,
                description: None,
            })
            .collect(),
        relations: vec![EntityRelationship {
            from_entity: "staging".into(),
            to_entity: "pcx-swap".into(),
            relation_type: RelationType::DependsOn,
            context: "deploys".into(),
        }],
        code_reference: None,
        author: None,
        derived_from: Vec::new(),
        meta: Default::default(),
    }
}

#[tokio::test]
async fn a_record_read_whole_shows_its_relations_and_what_ended_it() {
    let mut t = transports_with(true).await;
    let sid = t.memory.sessions.create(None, None).await.unwrap();
    let old = write(&t, sid, "Deploy on Fridays", "With pcx-swap.").await;
    let mut newer = request(sid, "Deploy on Mondays", "Fridays broke the weekend.");
    newer.meta.supersedes = vec![old.entry_id];
    let new = t.memory.records.write(newer).await.unwrap();
    let read = |id: uuid::Uuid| json!({ "session_id": sid.to_string(), "query_type": "record", "parameters": { "entry_id": id.to_string() } });

    let ended = call(&mut t, "query_conversation_context", read(old.entry_id)).await;
    assert_eq!(ended["end_reason"], json!("superseded"), "{ended}");
    assert_eq!(ended["ended_by"], json!(new.entry_id.to_string()));
    assert!(ended["valid_to"].is_string(), "{ended}");
    assert_eq!(
        ended["relations"],
        json!([{ "from_entity": "staging", "to_entity": "pcx-swap", "relation_type": "depends_on", "context": "deploys" }])
    );

    let current = call(&mut t, "query_conversation_context", read(new.entry_id)).await;
    assert_eq!(current["supersedes"], json!([old.entry_id.to_string()]));
    assert_eq!(current["ended_records"], json!([old.entry_id.to_string()]));
    assert!(current.get("end_reason").is_none(), "{current}");
    t.ct.cancel();
}

#[tokio::test]
async fn a_long_result_is_a_snippet_and_its_record_is_read_whole() {
    let mut t = transports_with(true).await;
    let sid = t.memory.sessions.create(None, None).await.unwrap();
    let filler = "The team reviewed unrelated chores and notes again. ".repeat(60);
    let description = format!("{filler}We deploy staging with pcx-swap every Friday. {filler}");
    write(&t, sid, "Chores", "The team tidied the notes.").await;
    let long_title = format!(
        "Deploys {}",
        "and the long story of every deploy we ever ran ".repeat(4)
    );
    let written = write(&t, sid, &long_title, &description).await;
    write(&t, sid, "Later chores", "The team tidied again.").await;
    let ask = |detail: Option<&str>| {
        let mut args = json!({ "query": "when do we deploy with pcx-swap", "scope": "session", "scope_id": sid.to_string(), "limit": 1 });
        if let Some(d) = detail {
            args["detail"] = json!(d);
        }
        args
    };

    let snippet = call(&mut t, "semantic_search", ask(None)).await;
    let hit = &snippet["results"][0];
    assert_eq!(hit["content_id"], json!(written.entry_id.to_string()));
    let text = hit["text_content"].as_str().unwrap();
    assert!(
        text.contains("We deploy staging with pcx-swap every Friday"),
        "{text}"
    );
    assert!(text.chars().count() < 600, "{} chars", text.chars().count());
    let full_chars = hit["full_chars"].as_u64().unwrap();
    assert!(full_chars > 5000, "{full_chars}");
    let title = hit["title"].as_str().unwrap();
    assert!(title.chars().count() <= 102, "{title}");
    let score = hit["combined_score"].as_f64().unwrap();
    assert_eq!((score * 1000.0).round() / 1000.0, score, "three decimals");
    assert_eq!(
        snippet["results"].as_array().unwrap().len(),
        1,
        "a snippet search shows the hits it found"
    );

    let full = call(&mut t, "semantic_search", ask(Some("full"))).await;
    let id = json!(written.entry_id.to_string());
    let found = full["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|h| h["content_id"] == id)
        .unwrap();
    let whole = found["text_content"].as_str().unwrap();
    assert_eq!(whole.chars().count() as u64, full_chars);
    assert!(found.get("full_chars").is_none());
    assert!(
        full["results"]
            .as_array()
            .unwrap()
            .iter()
            .any(|h| h.get("neighbour_of").is_some()),
        "a full search adds the records around each hit"
    );

    let refused = t
        .mcp
        .call_raw("semantic_search", ask(Some("everything")))
        .await;
    assert!(
        text_of(&refused).contains("detail is snippet or full"),
        "{refused}"
    );

    let record = call(
        &mut t,
        "query_conversation_context",
        json!({ "session_id": sid.to_string(), "query_type": "record", "parameters": { "entry_id": written.entry_id.to_string() } }),
    )
    .await;
    assert_eq!(record["description"], json!(description));
    assert_eq!(record["title"], json!(long_title));
    t.ct.cancel();
}
