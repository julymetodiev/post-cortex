// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The search tool's meta says whether a reranker ordered the hits, also
//! when a `select` keeps only part of the data (the cloud counts reranker
//! calls by it).

use std::sync::Arc;

use async_trait::async_trait;
use post_cortex_ports::Reranker;
use serde_json::json;

use super::*;
use crate::capabilities::records::good_request;
use crate::memory_system::backends_tests::{backends, embeddings_config, hash_engine};

/// Scores every document the same.
struct Even;

#[async_trait]
impl Reranker for Even {
    async fn score(
        &self,
        _query: &str,
        documents: &[Arc<str>],
    ) -> post_cortex_ports::error::Result<Vec<f32>> {
        Ok(vec![0.5; documents.len()])
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_selected_search_still_says_whether_it_was_reranked() {
    let backend = backends().await.into_iter().next().unwrap();
    let mut ports = backend.ports(Some(hash_engine()), None);
    ports.reranker = Some(Arc::new(Even));
    let memory = Memory::over_ports(ports, embeddings_config())
        .await
        .unwrap();
    let session = memory.sessions.create(None, None).await.unwrap();
    let mut req = good_request();
    req.session_id = session;
    req.content.title = "compaction stalls".into();
    memory.records.write(req).await.unwrap();
    assert!(
        memory
            .records
            .wait_idle(std::time::Duration::from_secs(60))
            .await
    );

    let search = |mode: &str| {
        json!({
            "query": "compaction",
            "scope": "session",
            "scope_id": session.to_string(),
            "mode": mode,
            "select": "$.results[*].content_id",
        })
    };
    let meta = |reranked: bool| {
        Some(serde_json::Map::from_iter([(
            RERANKED.to_string(),
            Value::Bool(reranked),
        )]))
    };

    let local = handle(&memory, search("local")).await.unwrap();
    assert!(
        local.data.as_ref().is_some_and(Value::is_array),
        "the selection: {:?}",
        local.data
    );
    assert_eq!(local.meta, meta(true), "local reranks");

    let fast = handle(&memory, search("fast")).await.unwrap();
    assert_eq!(fast.meta, meta(false), "fast does not");
}

/// A search whose reranker the cloud withheld for the plan says so, in its
/// data and its text; another value of the mark is refused.
#[tokio::test(flavor = "multi_thread")]
async fn a_search_withheld_by_the_plan_says_so() {
    let backend = backends().await.into_iter().next().unwrap();
    let mut ports = backend.ports(Some(hash_engine()), None);
    ports.reranker = Some(Arc::new(Even));
    let memory = Memory::over_ports(ports, embeddings_config())
        .await
        .unwrap();
    let session = memory.sessions.create(None, None).await.unwrap();
    let mut req = good_request();
    req.session_id = session;
    req.content.title = "compaction stalls".into();
    memory.records.write(req).await.unwrap();
    assert!(
        memory
            .records
            .wait_idle(std::time::Duration::from_secs(60))
            .await
    );
    let search = |withheld: &str| {
        json!({
            "query": "compaction",
            "scope": "session",
            "scope_id": session.to_string(),
            "mode": "fast",
            "rerank_withheld": withheld,
        })
    };

    let withheld = handle(&memory, search("plan_limit")).await.unwrap();
    let data = withheld.data.clone().unwrap();
    assert_eq!(
        (&data["reranked"], &data["rerank_skipped"]),
        (&json!(false), &json!("plan_limit"))
    );
    assert!(
        withheld.text.contains("Not reranked (plan_limit)"),
        "{}",
        withheld.text
    );
    assert!(handle(&memory, search("free")).await.is_err());
}
