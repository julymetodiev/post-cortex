// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;
use crate::capabilities::test_support::rocksdb_memory;
use crate::capabilities::{CreateWorkspace, Memory, ScopeError};
use crate::context_assembly::ContextSource;
use post_cortex_core::core::card::CardScope;
use post_cortex_core::services::UpdateCardRequest;
use post_cortex_core::workspace::SessionRole;

/// A code change (a kind no later record ends as a restatement, so records
/// that look alike all stay current).
async fn write(memory: &Memory, sid: Uuid, text: &str) {
    let mut req = crate::capabilities::records::good_request();
    req.interaction_type = post_cortex_core::core::context_update::UpdateType::CodeChanged;
    req.session_id = sid;
    req.content.title = text.to_string();
    req.content.description = text.to_string();
    memory.records.write(req).await.unwrap();
}

/// A memory with embeddings on: Assembly finds the records through Search.
async fn embedded() -> (Memory, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let memory = crate::memory_system::backends_tests::rocksdb_memory_in(
        &dir,
        crate::SystemConfig::default(),
    )
    .await
    .unwrap();
    (memory, dir)
}

/// Wait until every record written is embedded.
async fn embedded_all(memory: &Memory) {
    assert!(
        memory
            .records
            .wait_idle(std::time::Duration::from_secs(120))
            .await
    );
}

async fn card(memory: &Memory, scope: CardScope, text: &str) {
    memory
        .cards
        .update(UpdateCardRequest {
            scope,
            text: text.to_string(),
            expected_version: 0,
            author: None,
        })
        .await
        .unwrap();
}

/// The user card and the cards of the session's workspaces come first, in
/// the payload and the formatted text, and count against the budget.
#[tokio::test]
async fn cards_come_first() {
    let (memory, _dir) = embedded().await;
    let sid = memory.sessions.create(None, None).await.unwrap();
    write(&memory, sid, "zebracorn rollout plan").await;
    let create = |name: &str| CreateWorkspace {
        name: name.into(),
        description: String::new(),
    };
    let ws = memory.workspaces.create(create("p")).await.unwrap().id;
    memory
        .workspaces
        .add_session(ws, sid, SessionRole::Primary)
        .await
        .unwrap();
    let other = memory.workspaces.create(create("other")).await.unwrap().id;
    card(&memory, CardScope::User, "Answer briefly.").await;
    card(&memory, CardScope::Workspace(ws), "## Stack\nrust").await;
    card(&memory, CardScope::Workspace(other), "not this project").await;
    embedded_all(&memory).await;

    let ctx = memory
        .assembly
        .assemble(AssembleRequest {
            query: "zebracorn".into(),
            target: Target::Session(sid),
            token_budget: 0,
            corpus: Corpus::Records,
            person: None,
        })
        .await
        .unwrap();
    let texts: Vec<&str> = ctx.cards.iter().map(|c| c.text.as_str()).collect();
    assert_eq!(texts, ["Answer briefly.", "## Stack\nrust"]);
    let formatted = context_assembly::format_for_llm(&ctx);
    assert!(
        formatted.starts_with("User card:\nAnswer briefly.\n\nProject card:\n## Stack\nrust"),
        "{formatted}"
    );
    assert!(formatted.contains("zebracorn rollout plan"));
    assert!(ctx.total_tokens >= context_assembly::card_tokens(&ctx.cards));
}

/// One scope behaviour (review W4 item 9): an unknown session or workspace
/// is NotFound, neither named is Missing, and a workspace whose member was
/// deleted assembles from the members that are left.
#[tokio::test]
async fn scope_failures_are_one_behaviour() {
    let (memory, _dir) = embedded().await;
    let assemble = |target| {
        memory.assembly.assemble(AssembleRequest {
            query: "zebracorn".into(),
            target,
            token_budget: 0,
            corpus: Corpus::Records,
            person: None,
        })
    };
    for target in [
        Target::Session(Uuid::new_v4()),
        Target::Workspace(Uuid::new_v4()),
    ] {
        assert!(matches!(
            assemble(target).await,
            Err(AssemblyError::Scope(ScopeError::NotFound(_)))
        ));
    }
    assert!(matches!(Target::of(None, None), Err(ScopeError::Missing)));
    assert_eq!(
        Target::of(Some(Uuid::nil()), Some(Uuid::max())).unwrap(),
        Target::Workspace(Uuid::max()),
        "the workspace wins"
    );

    let ws = memory
        .workspaces
        .create(CreateWorkspace {
            name: "ws".into(),
            description: String::new(),
        })
        .await
        .unwrap()
        .id;
    let kept = memory.sessions.create(None, None).await.unwrap();
    let gone = memory.sessions.create(None, None).await.unwrap();
    for sid in [kept, gone] {
        memory
            .workspaces
            .add_session(ws, sid, SessionRole::Primary)
            .await
            .unwrap();
    }
    write(&memory, kept, "zebracorn kept").await;
    write(&memory, gone, "zebracorn gone").await;
    assert!(memory.sessions.delete(gone).await.unwrap());
    embedded_all(&memory).await;

    let ctx = assemble(Target::Workspace(ws)).await.unwrap();
    let text = context_assembly::format_for_llm(&ctx);
    assert!(text.contains("zebracorn kept"), "{text}");
    assert!(!text.contains("zebracorn gone"), "{text}");
}

/// With records and messages, the excerpts take at most half the budget
/// and the records the rest; with no matching turn the records get all of
/// it.
#[tokio::test]
async fn all_sources_share_the_budget() {
    use post_cortex_core::core::message::{MessageRole, NewMessage};
    use post_cortex_core::services::AppendMessagesRequest;

    let (memory, _dir) = embedded().await;
    let sid = memory.sessions.create(None, None).await.unwrap();
    for i in 0..6 {
        let text = format!("zebracorn rollout note {i}: {}", "details ".repeat(60));
        write(&memory, sid, &text).await;
    }
    let assemble_now = |corpus| {
        memory.assembly.assemble(AssembleRequest {
            query: "zebracorn rollout".into(),
            target: Target::Session(sid),
            token_budget: 600,
            corpus,
            person: None,
        })
    };
    let count = |ctx: &AssembledContext, conversation: bool| {
        ctx.items
            .iter()
            .filter(|i| (i.source == ContextSource::Conversation) == conversation)
            .count()
    };

    let records = assemble_now(Corpus::Records).await.unwrap();
    let all_without_turns = assemble_now(Corpus::All).await.unwrap();
    assert!(count(&records, false) >= 2, "{records:?}");
    assert_eq!(count(&all_without_turns, false), count(&records, false));

    let turn = |id: &str, text: &str| NewMessage {
        turn_id: id.to_string(),
        role: MessageRole::User,
        author: None,
        source_session: None,
        timestamp: chrono::Utc::now(),
        text: text.to_string(),
        tools: Vec::new(),
        files: Vec::new(),
        parent: None,
        blocks: Vec::new(),
    };
    memory
        .messages
        .append(AppendMessagesRequest {
            session_id: sid,
            messages: vec![
                turn("t1", "when does the zebracorn rollout start?"),
                turn("t2", "the zebracorn rollout starts on monday"),
            ],
        })
        .await
        .unwrap();
    // The turns are embedded by the pipeline, not by the append.
    assert!(
        memory
            .records
            .wait_idle(std::time::Duration::from_secs(120))
            .await
    );
    let all = assemble_now(Corpus::All).await.unwrap();
    assert!(count(&all, true) >= 1, "{all:?}");
    assert!(count(&all, false) >= 1, "records keep the rest: {all:?}");
    assert!(all.total_tokens <= 600, "{}", all.total_tokens);

    let messages = assemble_now(Corpus::Messages).await.unwrap();
    assert_eq!(count(&messages, false), 0);
    assert!(count(&messages, true) >= 1);
}

/// A query that names a hub with many relationships: the graph takes at
/// most half of the budget, the records still fit, and the whole stays
/// within the budget.
#[tokio::test]
async fn a_hub_in_the_query_leaves_the_records_room() {
    use post_cortex_core::core::context_update::{EntityRelationship, EntityType, RelationType};

    let (memory, _dir) = embedded().await;
    let sid = memory.sessions.create(None, None).await.unwrap();
    for i in 0..4 {
        write(&memory, sid, &format!("zebracorn rollout record {i}")).await;
    }
    embedded_all(&memory).await;

    let mut graph = SimpleEntityGraph::new();
    let names: Vec<String> = (0..30).map(|i| format!("component{i}")).collect();
    for name in &names {
        graph.add_or_update_entity(name.clone(), EntityType::Concept, chrono::Utc::now(), "t");
    }
    // Four relations each: the whole relationships section takes ~1,300
    // tokens.
    for (i, from) in names.iter().enumerate() {
        for step in 1..=4 {
            graph.add_relationship(
                EntityRelationship {
                    from_entity: from.clone(),
                    to_entity: names[(i + step) % names.len()].clone(),
                    relation_type: RelationType::RelatedTo,
                    context: String::new(),
                },
                chrono::Utc::now(),
            );
        }
    }
    let query = format!("zebracorn rollout {}", names.join(" "));
    let whole = context_assembly::graph_context(&query, &graph, usize::MAX).tokens;
    assert!((1000..2000).contains(&whole), "{whole}");

    let ctx = memory
        .assembly
        .assemble_over(
            &query,
            AssemblyScope {
                graph: &graph,
                search: SearchScope::Session(sid),
                sessions: &[sid],
                workspaces: &[],
                person: None,
            },
            1200,
            Corpus::Records,
        )
        .await;
    let graph_tokens = ctx.total_tokens - ctx.items.iter().map(|i| i.token_estimate).sum::<usize>();
    assert!(graph_tokens <= 600, "{graph_tokens}");
    assert!(!ctx.items.is_empty(), "{ctx:?}");
    assert!(ctx.total_tokens <= 1200, "{}", ctx.total_tokens);
}

/// The records come from Search: one written in other words than the query
/// is found, a record that no longer holds is left out, and with
/// embeddings off the cards and the graph still come, without records.
#[tokio::test]
async fn records_come_from_search_and_only_current_ones() {
    let (memory, _dir) = embedded().await;
    let sid = memory.sessions.create(None, None).await.unwrap();
    write(
        &memory,
        sid,
        "launch of the zebracorn: first monday of june",
    )
    .await;
    let mut old = crate::capabilities::records::good_request();
    old.session_id = sid;
    old.content.title = "zebracorn rollout starts on friday".into();
    old.content.description = "zebracorn rollout starts on friday".into();
    let old = memory.records.write(old).await.unwrap();
    let mut new = crate::capabilities::records::good_request();
    new.session_id = sid;
    new.content.title = "zebracorn rollout moved to sunday".into();
    new.content.description = "zebracorn rollout moved to sunday".into();
    new.meta.supersedes = vec![old.entry_id];
    memory.records.write(new).await.unwrap();
    embedded_all(&memory).await;

    let ctx = memory
        .assembly
        .assemble(AssembleRequest {
            query: "when does the zebracorn rollout begin?".into(),
            target: Target::Session(sid),
            token_budget: 0,
            corpus: Corpus::Records,
            person: None,
        })
        .await
        .unwrap();
    let texts: Vec<&str> = ctx.items.iter().map(|i| i.text.as_str()).collect();
    assert!(texts.iter().any(|t| t.contains("monday")), "{texts:?}");
    assert!(texts.iter().any(|t| t.contains("sunday")), "{texts:?}");
    assert!(
        !texts.iter().any(|t| t.contains("friday")),
        "ended: {texts:?}"
    );
    assert!(
        ctx.items
            .iter()
            .all(|i| i.source == ContextSource::SemanticMatch)
    );

    let (off, _off_dir) = rocksdb_memory().await;
    let sid = off.sessions.create(None, None).await.unwrap();
    write(&off, sid, "zebracorn rollout plan").await;
    card(&off, CardScope::User, "Answer briefly.").await;
    let ctx = off
        .assembly
        .assemble(AssembleRequest {
            query: "zebracorn".into(),
            target: Target::Session(sid),
            token_budget: 0,
            corpus: Corpus::Records,
            person: None,
        })
        .await
        .unwrap();
    assert!(ctx.items.is_empty(), "{ctx:?}");
    assert_eq!(ctx.cards.len(), 1);
}

/// An empty or blank query is refused by the service itself, so every
/// transport (MCP, gRPC, the query alias) refuses it the same way.
#[tokio::test]
async fn an_empty_query_is_invalid() {
    let (memory, _dir) = crate::capabilities::test_support::rocksdb_memory().await;
    let sid = memory.sessions.create(None, None).await.unwrap();
    for query in ["", "   "] {
        let err = memory
            .assembly
            .assemble(AssembleRequest {
                query: query.into(),
                target: Target::Session(sid),
                token_budget: 0,
                corpus: Corpus::Records,
                person: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, AssemblyError::InvalidArgument(_)), "{err:?}");
    }
}
