// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The detail views over a real memory instance: the global scope, the
//! overview's stored numbers, pages of a stream, and their errors.

use std::collections::BTreeSet;

use super::*;
use crate::capabilities::records::good_request;
use crate::capabilities::test_support::rocksdb_memory;
use crate::capabilities::{Memory, ScopeError};
use crate::graph_views::detail::{PageAt, RecordFilter};
use post_cortex_core::core::message::{MessageRole, NewMessage};
use post_cortex_core::services::AppendMessagesRequest;

async fn session_with(memory: &Memory, titles: &[&str]) -> Uuid {
    let sid = memory.sessions.create(None, None).await.unwrap();
    for title in titles {
        let mut req = good_request();
        req.session_id = sid;
        req.content.title = title.to_string();
        memory.records.write(req).await.unwrap();
    }
    sid
}

async fn turns(memory: &Memory, sid: Uuid, n: usize) {
    let messages = (0..n)
        .map(|i| NewMessage {
            turn_id: format!("turn-{i}"),
            role: MessageRole::User,
            author: Some(if i % 2 == 0 { "ana" } else { "bob" }.into()),
            source_session: None,
            timestamp: Utc::now(),
            text: format!("message {i}"),
            tools: Vec::new(),
            files: Vec::new(),
            parent: None,
            blocks: Vec::new(),
        })
        .collect();
    memory
        .messages
        .append(AppendMessagesRequest {
            session_id: sid,
            messages,
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn the_global_scope_reads_every_session() {
    let (memory, _dir) = rocksdb_memory().await;
    let a = session_with(&memory, &["one", "two"]).await;
    let b = session_with(&memory, &["three"]).await;
    let views = &memory.detail_views;

    let all = views
        .records(Target::Global, RecordFilter::default(), None, None)
        .await
        .unwrap();
    assert_eq!(all.total, 3);
    let sessions: BTreeSet<Uuid> = all.records.iter().map(|r| r.session_id).collect();
    assert_eq!(sessions, BTreeSet::from([a, b]));

    let one = views
        .records(Target::Session(b), RecordFilter::default(), None, None)
        .await
        .unwrap();
    assert_eq!(one.total, 1);
    let overview = views.overview(Target::Global, None, None).await.unwrap();
    assert_eq!((overview.records, overview.sessions), (3, 2));
}

#[tokio::test]
async fn the_overview_counts_stored_messages() {
    let (memory, _dir) = rocksdb_memory().await;
    let sid = session_with(&memory, &["one"]).await;
    turns(&memory, sid, 3).await;
    let overview = memory
        .detail_views
        .overview(Target::Session(sid), None, None)
        .await
        .unwrap();
    assert_eq!(overview.messages, 3);
    assert!(overview.last_activity.is_some());
}

#[tokio::test]
async fn conversations_page_a_stream_both_ways() {
    let (memory, _dir) = rocksdb_memory().await;
    let sid = session_with(&memory, &["one"]).await;
    turns(&memory, sid, 5).await;
    let views = &memory.detail_views;

    let list = views
        .conversations(Target::Session(sid), None, None, None)
        .await
        .unwrap();
    assert_eq!((list.total, list.next), (1, None));
    assert_eq!(list.conversations[0].messages, 5);
    assert_eq!(list.conversations[0].authors, ["ana", "bob"]);

    let first = views
        .conversation(sid, PageAt::From(0), Some(2))
        .await
        .unwrap();
    let seqs: Vec<u64> = first.turns.iter().map(|t| t.message.seq).collect();
    assert_eq!(seqs, [0, 1]);
    assert_eq!((first.next, first.prev, first.total), (Some(2), None, 5));

    let newest = views
        .conversation(sid, PageAt::Before(u64::MAX), Some(2))
        .await
        .unwrap();
    let seqs: Vec<u64> = newest.turns.iter().map(|t| t.message.seq).collect();
    assert_eq!(seqs, [3, 4], "in stream order");
    assert_eq!((newest.next, newest.prev), (None, Some(3)));
}

#[tokio::test]
async fn conversations_page_most_recently_active_first() {
    let (memory, _dir) = rocksdb_memory().await;
    let mut sessions = Vec::new();
    for title in ["a", "b", "c"] {
        let sid = session_with(&memory, &[title]).await;
        turns(&memory, sid, 1).await;
        sessions.push(sid);
    }
    session_with(&memory, &["no turns"]).await;
    let views = &memory.detail_views;
    let first = views
        .conversations(Target::Global, Some(2), None, None)
        .await
        .unwrap();
    assert_eq!(first.total, 3, "streams without turns are left out");
    let mut seen: Vec<Uuid> = first.conversations.iter().map(|c| c.session_id).collect();
    let second = views
        .conversations(Target::Global, Some(2), first.next, None)
        .await
        .unwrap();
    assert_eq!(second.next, None);
    seen.extend(second.conversations.iter().map(|c| c.session_id));
    assert_eq!(seen.len(), 3, "no stream repeats");
    assert_eq!(
        seen.iter().collect::<BTreeSet<_>>(),
        sessions.iter().collect::<BTreeSet<_>>()
    );
}

#[tokio::test]
async fn missing_things_are_typed_errors() {
    let (memory, _dir) = rocksdb_memory().await;
    let sid = session_with(&memory, &["one"]).await;
    let views = &memory.detail_views;
    let missing = Uuid::new_v4();
    assert!(matches!(
        views.record(Target::Session(sid), missing).await,
        Err(ViewsError::RecordNotFound(id)) if id == missing
    ));
    assert!(matches!(
        views.graph(Target::Session(missing), None, None).await,
        Err(ViewsError::Scope(ScopeError::NotFound(_)))
    ));
    let blank = EntityRequest {
        name: "  ".into(),
        ..Default::default()
    };
    assert!(matches!(
        views.entity(Target::Session(sid), blank).await,
        Err(ViewsError::InvalidArgument(_))
    ));
    let nobody = EntityRequest {
        name: "nobody".into(),
        ..Default::default()
    };
    assert!(matches!(
        views.entity(Target::Session(sid), nobody).await,
        Err(ViewsError::View(ViewError::EntityNotFound(_)))
    ));
    assert!(matches!(
        views
            .file(Target::Session(sid), " ".into(), None, None)
            .await,
        Err(ViewsError::InvalidArgument(_))
    ));
}

/// Listed by a speaker (W25): only the agent's conversations, each with
/// who asked, its first question and how it stands.
#[tokio::test]
async fn an_agents_conversations_say_who_asked_and_how_they_stand() {
    use post_cortex_core::core::conversation::ConversationState;
    use post_cortex_core::core::message::MessageBlock;

    let (memory, _dir) = rocksdb_memory().await;
    let talk = memory.sessions.create(None, None).await.unwrap();
    let other = session_with(&memory, &["one"]).await;
    turns(&memory, other, 2).await;
    let said = |turn: &str, role, author: &str, blocks: Vec<MessageBlock>, text: &str| NewMessage {
        turn_id: turn.into(),
        role,
        author: Some(author.into()),
        source_session: None,
        timestamp: Utc::now(),
        text: text.into(),
        tools: Vec::new(),
        files: Vec::new(),
        parent: None,
        blocks,
    };
    memory
        .messages
        .append(AppendMessagesRequest {
            session_id: talk,
            messages: vec![
                said(
                    "p",
                    MessageRole::User,
                    "julius",
                    Vec::new(),
                    "what is left?",
                ),
                said(
                    "a",
                    MessageRole::Assistant,
                    "chief",
                    Vec::new(),
                    "two tasks",
                ),
                said(
                    "e",
                    MessageRole::Assistant,
                    "chief",
                    vec![MessageBlock::End {
                        stop: "end_turn".into(),
                        detail: String::new(),
                    }],
                    "",
                ),
            ],
        })
        .await
        .unwrap();
    let listed = memory
        .detail_views
        .conversations(Target::Global, None, None, Some("chief"))
        .await
        .unwrap();
    assert_eq!(listed.total, 1, "only the chief's");
    let chat = &listed.conversations[0];
    assert_eq!(chat.session_id, talk);
    assert_eq!(
        (
            chat.asker.as_deref(),
            chat.first_question.as_deref(),
            chat.state
        ),
        (
            Some("julius"),
            Some("what is left?"),
            Some(ConversationState::Idle)
        )
    );
    let every = memory
        .detail_views
        .conversations(Target::Global, None, None, None)
        .await
        .unwrap();
    assert_eq!(every.total, 2);
    assert!(every.conversations.iter().all(|c| c.state.is_none()));
}

/// Scores every document the same: a reranker that is there.
struct SameScore;

#[async_trait::async_trait]
impl post_cortex_ports::Reranker for SameScore {
    async fn score(
        &self,
        _query: &str,
        documents: &[Arc<str>],
    ) -> post_cortex_ports::error::Result<Vec<f32>> {
        Ok(vec![0.5; documents.len()])
    }
}

/// A search says whether a reranker ordered it: a `local` search where no
/// reranker is configured is not reranked (the cloud's tenants have none),
/// `fast` never is.
#[tokio::test(flavor = "multi_thread")]
async fn a_search_says_whether_it_was_reranked() {
    use crate::content_vectorizer::{SearchMode, SearchOptions};
    use crate::memory_system::backends_tests::{backends, embeddings_config, hash_engine};

    let in_mode = |mode| SearchOptions {
        mode: Some(mode),
        ..Default::default()
    };
    let backend = backends().await.into_iter().next().unwrap();
    for (reranker, local) in [
        (None, false),
        (
            Some(Arc::new(SameScore) as Arc<dyn post_cortex_ports::Reranker>),
            true,
        ),
    ] {
        let mut ports = backend.ports(Some(hash_engine()), None);
        ports.reranker = reranker;
        let memory = Memory::over_ports(ports, embeddings_config())
            .await
            .unwrap();
        session_with(&memory, &["compaction stalls"]).await;
        assert!(
            memory
                .records
                .wait_idle(std::time::Duration::from_secs(60))
                .await
        );
        let views = &memory.detail_views;
        let found = views
            .search(Target::Global, "compaction", in_mode(SearchMode::Local))
            .await
            .unwrap();
        assert_eq!((found.hits.is_empty(), found.reranked), (false, local));
        let fast = views
            .search(Target::Global, "compaction", in_mode(SearchMode::Fast))
            .await
            .unwrap();
        assert!(!fast.reranked);
    }
}

/// Counts the searches it reranks: the remote reranker of a test.
#[derive(Default)]
struct Remote(std::sync::atomic::AtomicUsize);

#[async_trait::async_trait]
impl post_cortex_ports::Reranker for Remote {
    async fn score(
        &self,
        _query: &str,
        documents: &[Arc<str>],
    ) -> post_cortex_ports::error::Result<Vec<f32>> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(vec![0.5; documents.len()])
    }
}

/// An accurate search reaches the remote reranker only when every
/// workspace of its scope allows it: not in a new workspace, then in the
/// workspace and its session once a person allows it, never for a session
/// outside workspaces or globally while another workspace forbids it.
#[tokio::test(flavor = "multi_thread")]
async fn only_workspaces_that_allow_it_reach_the_remote_reranker() {
    use crate::capabilities::{CreateWorkspace, UpdateWorkspace};
    use crate::content_vectorizer::{SearchMode, SearchOptions};
    use crate::memory_system::backends_tests::{backends, embeddings_config, hash_engine};
    use post_cortex_core::workspace::SessionRole;
    use std::sync::atomic::Ordering;

    let accurate = || SearchOptions {
        mode: Some(SearchMode::Accurate),
        ..Default::default()
    };
    let backend = backends().await.into_iter().next().unwrap();
    let remote = Arc::new(Remote::default());
    let mut ports = backend.ports(Some(hash_engine()), None);
    ports.accurate_reranker = Some(Arc::clone(&remote) as Arc<dyn post_cortex_ports::Reranker>);
    let memory = Memory::over_ports(ports, embeddings_config())
        .await
        .unwrap();
    let create = |name: &str| CreateWorkspace {
        name: name.into(),
        description: String::new(),
    };
    let ws = memory.workspaces.create(create("allows")).await.unwrap().id;
    let other = memory
        .workspaces
        .create(create("forbids"))
        .await
        .unwrap()
        .id;
    let member = session_with(&memory, &["compaction stalls"]).await;
    let outside = session_with(&memory, &["compaction again"]).await;
    memory
        .workspaces
        .add_session(ws, member, SessionRole::Primary)
        .await
        .unwrap();
    assert!(
        memory
            .records
            .wait_idle(std::time::Duration::from_secs(60))
            .await
    );
    let views = &memory.detail_views;
    let search = |target| views.search(target, "compaction", accurate());

    let found = search(Target::Workspace(ws)).await.unwrap();
    assert_eq!((found.hits.is_empty(), found.reranked), (false, false));
    assert_eq!(
        remote.0.load(Ordering::SeqCst),
        0,
        "a new workspace forbids it"
    );

    let allowed = UpdateWorkspace {
        remote_rerank: Some(true),
        ..UpdateWorkspace::default()
    };
    let updated = memory.workspaces.update(ws, allowed).await.unwrap();
    assert!(updated.remote_rerank());
    assert!(search(Target::Workspace(ws)).await.unwrap().reranked);
    assert!(search(Target::Session(member)).await.unwrap().reranked);
    let reached = remote.0.load(Ordering::SeqCst);
    assert_eq!(reached, 2);

    assert!(!search(Target::Session(outside)).await.unwrap().reranked);
    assert!(!search(Target::Global).await.unwrap().reranked);
    assert_eq!(
        remote.0.load(Ordering::SeqCst),
        reached,
        "{other} forbids it"
    );
}
