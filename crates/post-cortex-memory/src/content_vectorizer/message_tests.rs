// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Messages and authors: messages are searched only when asked for,
//! records filter and rank by author, incomplete message vectors are
//! embedded again, multibyte text is extracted safely.

use super::{ContentKind, ContentVectorizer, PREFER_AUTHOR_BOOST, SearchOptions};
use crate::content_vectorizer::VectorScope;
use crate::content_vectorizer::tests::DIM;
use crate::content_vectorizer::tests::session_with;
use crate::content_vectorizer::tests::update;
use crate::content_vectorizer::tests::vectorizer;
use crate::memory_system::VectorizedIds;
use chrono::Utc;
use post_cortex_core::core::message::{Message, MessageRole, NewMessage};
use post_cortex_core::session::active_session::ActiveSession;
use post_cortex_ports::{Corpus, MESSAGE_CONTENT_TYPE, VectorMetadata};
use uuid::Uuid;

fn chat(session_id: Uuid, seq: u64, text: &str) -> Message {
    NewMessage {
        turn_id: format!("turn-{seq}"),
        role: if seq.is_multiple_of(2) {
            MessageRole::User
        } else {
            MessageRole::Assistant
        },
        author: None,
        source_session: None,
        timestamp: Utc::now(),
        text: text.to_string(),
        tools: Vec::new(),
        files: Vec::new(),
        parent: None,
        blocks: Vec::new(),
    }
    .into_message(session_id, seq)
}

/// Messages are embedded as `Message` vectors under `message:<seq>`, are
/// left out of searches unless asked for, and do not count as vectorized
/// updates of the session.
#[tokio::test]
async fn messages_are_searched_only_when_asked_for() {
    let marks = VectorizedIds::default();
    let session = session_with(&[("zebracorn migration plan", "zebra")]).await;
    let (v, storage) = vectorizer().await;
    v.vectorize_session(&session, &marks).await.unwrap();
    let messages = [
        chat(session.id(), 0, "what happened to the zebracorn yesterday?"),
        chat(session.id(), 1, "the zebracorn import failed on a timeout"),
    ];
    assert_eq!(v.vectorize_messages(&messages).await.unwrap(), 2);
    assert_eq!(
        v.vectorize_messages(&messages).await.unwrap(),
        0,
        "turns with vectors are skipped"
    );
    assert!(
        storage
            .rows
            .lock()
            .contains_key(&(session.id().to_string(), "message:1".to_string())),
        "message vectors are persisted"
    );
    assert!(
        !v.needs_vectorization(&session),
        "messages are not updates waiting for vectors"
    );

    let scope = [session.id()];
    let search = |corpus: Corpus| {
        v.search(
            "zebracorn",
            10,
            VectorScope::Sessions(&scope),
            SearchOptions {
                recency_bias: Some(0.0),
                corpus,
                ..SearchOptions::default()
            },
        )
    };
    let records = search(Corpus::Records).await.unwrap();
    assert!(!records.is_empty());
    assert!(
        records
            .iter()
            .all(|r| r.content_type != ContentKind::Message),
        "{records:?}"
    );
    let mut ids: Vec<String> = search(Corpus::Messages)
        .await
        .unwrap()
        .iter()
        .inspect(|r| assert_eq!(r.content_type, ContentKind::Message))
        .map(|r| r.content_id.clone())
        .collect();
    ids.sort();
    assert_eq!(ids, ["message:0", "message:1"]);
    assert_eq!(search(Corpus::All).await.unwrap().len(), records.len() + 2);

    v.remove_contents(session.id(), &["message:0".to_string()])
        .await
        .unwrap();
    let left = search(Corpus::Messages).await.unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].content_id, "message:1");

    // A turn whose vector is missing is embedded again; the other is not.
    assert_eq!(v.vectorize_messages(&messages).await.unwrap(), 1);
    assert_eq!(search(Corpus::Messages).await.unwrap().len(), 2);
}

/// Records carry their author into the vector index: results show it and a
/// search can keep one author's records only.
#[tokio::test]
async fn records_are_filtered_by_author() {
    let marks = VectorizedIds::default();
    let mut session = ActiveSession::new(Uuid::new_v4(), None, None, chrono::Utc::now());
    for (title, author) in [
        ("zebracorn rollout plan by alice", Some("alice")),
        ("zebracorn rollback notes by bob", Some("bob")),
        ("zebracorn history from before authorship", None),
    ] {
        let mut u = update(title, "zebra");
        u.author = author.map(String::from);
        session.apply_record(u);
    }
    let (v, _) = vectorizer().await;
    v.vectorize_session(&session, &marks).await.unwrap();

    let scope = [session.id()];
    let search = |author: Option<&str>| {
        v.search(
            "zebracorn",
            10,
            VectorScope::Sessions(&scope),
            SearchOptions {
                recency_bias: Some(0.0),
                author: author.map(String::from),
                ..SearchOptions::default()
            },
        )
    };
    let all = search(None).await.unwrap();
    let mut authors: Vec<Option<String>> = all
        .iter()
        .filter(|r| r.content_type != ContentKind::EntityDescription)
        .map(|r| r.author.clone())
        .collect();
    authors.sort();
    assert_eq!(authors, [None, Some("alice".into()), Some("bob".into())]);

    let alice = search(Some("alice")).await.unwrap();
    assert_eq!(alice.len(), 1, "{alice:?}");
    assert!(alice[0].text_content.contains("by alice"));
    assert!(search(Some("carol")).await.unwrap().is_empty());
}

/// `prefer_author` lifts that author's results by `PREFER_AUTHOR_BOOST`
/// and keeps everyone else's.
#[tokio::test]
async fn preferred_author_ranks_higher() {
    let marks = VectorizedIds::default();
    let mut session = ActiveSession::new(Uuid::new_v4(), None, None, chrono::Utc::now());
    for (title, author) in [
        ("quokka migration plan by alice", Some("alice")),
        ("quokka migration plan by bob", Some("bob")),
    ] {
        let mut u = update(title, "quokka");
        u.author = author.map(String::from);
        session.apply_record(u);
    }
    let (v, _) = vectorizer().await;
    v.vectorize_session(&session, &marks).await.unwrap();

    let scope = [session.id()];
    let search = |prefer: &str| {
        v.search(
            "quokka migration plan",
            10,
            VectorScope::Sessions(&scope),
            SearchOptions {
                recency_bias: Some(0.0),
                prefer_author: Some(prefer.to_string()),
                ..SearchOptions::default()
            },
        )
    };
    for (prefer, other) in [("alice", "bob"), ("bob", "alice")] {
        let hits = search(prefer).await.unwrap();
        let score = |who: &str| {
            hits.iter()
                .find(|r| r.author.as_deref() == Some(who))
                .map(|r| r.combined_score)
                .unwrap_or_else(|| panic!("no hit by {who}: {hits:?}"))
        };
        let lead = score(prefer) - score(other);
        assert!(
            lead > PREFER_AUTHOR_BOOST / 2.0,
            "{prefer} leads {other} by only {lead}"
        );
        let first = hits
            .iter()
            .find(|r| r.content_type != ContentKind::EntityDescription)
            .unwrap();
        assert_eq!(first.author.as_deref(), Some(prefer));
    }
}

/// A long message is embedded as chunks; it is skipped again only while
/// its stored vectors are exactly those chunks: a missing chunk, or the
/// vectors of another text at its position, make it embedded again, with
/// the stale vectors removed from memory and storage. A turn too short to
/// embed loses the vectors left at its position.
#[tokio::test]
async fn messages_with_incomplete_vectors_are_embedded_again() {
    let session = Uuid::new_v4();
    let (v, storage) = vectorizer().await;
    let long: String = (0..300)
        .map(|i| format!("long turn sentence {i}. "))
        .collect();
    let messages = [chat(session, 0, &long), chat(session, 1, "ok")];
    let chunks = v.vectorize_messages(&messages).await.unwrap();
    assert!(chunks > 2, "long turn split into chunks");
    assert_eq!(v.vectorize_messages(&messages).await.unwrap(), 0);
    let whole = stored(&v, session).await;
    assert_eq!(whole.len(), chunks);

    // A chunk missing.
    v.remove_contents(session, &["message:0#2".to_string()])
        .await
        .unwrap();
    assert_eq!(v.vectorize_messages(&messages).await.unwrap(), chunks);
    assert_eq!(stored(&v, session).await, whole);
    assert_eq!(storage.len(), chunks);

    // Another text's vectors at both positions.
    v.remove_contents(session, &["message:0".to_string()])
        .await
        .unwrap();
    for (id, text) in [
        ("message:0", "another stream's turn zero"),
        ("message:1", "another stream's turn one"),
    ] {
        let meta = VectorMetadata::new(
            id.to_string(),
            text.to_string(),
            session.to_string(),
            MESSAGE_CONTENT_TYPE.to_string(),
        );
        v.store_vectors(vec![post_cortex_ports::IndexEntry {
            vector: vec![0.5; DIM],
            meta,
        }])
        .await;
    }
    assert_eq!(v.vectorize_messages(&messages).await.unwrap(), chunks);
    assert_eq!(
        stored(&v, session).await,
        whole,
        "the chunks are back, the short turn has none"
    );
    assert_eq!(storage.len(), chunks);
}

/// The content ids of the entries of `session`, sorted.
async fn stored(v: &ContentVectorizer, session: Uuid) -> Vec<String> {
    let mut ids: Vec<String> = v
        .index()
        .entries(&session.to_string(), &post_cortex_ports::EntrySelect::All)
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.meta.id)
        .collect();
    ids.sort();
    ids
}

/// Text extraction logs a preview at debug level; a title or description
/// of multibyte text longer than the preview must not panic there.
#[test]
fn multibyte_text_is_extracted_with_debug_logging_on() {
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_test_writer()
        .finish();
    let title = "Решение: съхраняваме картите в RocksDB заради скоростта";
    let record = update(title, "карти");
    assert!(title.len() > 50 && !title.is_char_boundary(50));

    let text = tracing::subscriber::with_default(subscriber, || {
        super::vector_text::extract_text_from_update(&record)
    });

    assert!(text.contains(title));
}
