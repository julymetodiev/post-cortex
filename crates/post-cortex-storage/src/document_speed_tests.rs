// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The speed of a filtered list of documents, before and after their keys
//! (docs/memory.md, document keys and pages; task `f47c5546`), on both
//! stores: a workflow's journal of 500 runs of 20 steps, the steps of one
//! run read. Before, memory read every latest version of the kind
//! (`latest_documents`) and filtered them itself, decoding each, which is
//! not counted here, so the gain is at least what is printed. After, a
//! page of the run's key. Run by hand:
//! `scripts/cargo.sh ignored speed_of_a_run_s_steps`.

use std::time::Instant;

use chrono::Utc;
use uuid::Uuid;

use post_cortex_core::core::document::{DocumentData, DocumentDraft, DocumentKey, DocumentKind};
use post_cortex_ports::storage::{DocumentStore, LatestQuery};

/// Runs of the journal.
const RUNS: usize = 500;

/// Steps of each run.
const STEPS: usize = 20;

/// Reads timed, after one to warm up.
const READS: u32 = 20;

/// The key of run `run`, as memory makes it of a step's `run` field.
fn run_key(run: usize) -> DocumentKey {
    let mut digest = [0u8; 16];
    digest[..8].copy_from_slice(&(run as u64).to_be_bytes());
    DocumentKey::new(1, digest)
}

async fn timed(store: &dyn DocumentStore, name: &str) {
    let ws = Uuid::new_v4();
    let kind: DocumentKind = "workflow/step".parse().unwrap();
    let started = Instant::now();
    for run in 0..RUNS {
        for step in 0..STEPS {
            let draft = DocumentDraft {
                workspace_id: ws,
                id: Uuid::new_v4(),
                kind: kind.clone(),
                data: DocumentData::Message {
                    type_name: "pcx.workflow.v1.Step".into(),
                    bytes: vec![7; 200 + step],
                },
                title: format!("run {run} step {step}"),
                text: String::new(),
                author: None,
                keys: vec![run_key(run), DocumentKey::new(2, [step as u8; 16])],
            };
            store.put_document(draft, 0).await.unwrap();
        }
    }
    let wrote = started.elapsed();

    let before = || async {
        let all = store.latest_documents(ws, Some(&kind)).await.unwrap();
        assert_eq!(all.len(), RUNS * STEPS);
    };
    let keys = [run_key(RUNS / 2)];
    let after = || async {
        let page = store
            .latest_page(LatestQuery {
                workspace_id: ws,
                kind: &kind,
                keys: &keys,
                after: None,
                limit: 100,
            })
            .await
            .unwrap();
        assert_eq!((page.documents.len(), page.total), (STEPS, STEPS));
    };
    before().await;
    let at = Instant::now();
    for _ in 0..READS {
        before().await;
    }
    let before_each = at.elapsed() / READS;
    after().await;
    let at = Instant::now();
    for _ in 0..READS {
        after().await;
    }
    let after_each = at.elapsed() / READS;
    println!(
        "{name}: {} documents written in {wrote:?}; a run's steps: before {before_each:?}, \
         after {after_each:?} ({:.0}x), at {}",
        RUNS * STEPS,
        before_each.as_secs_f64() / after_each.as_secs_f64(),
        Utc::now().to_rfc3339()
    );
}

#[tokio::test]
#[ignore = "a measurement, run by hand"]
async fn speed_of_a_run_s_steps() {
    let dir = tempfile::tempdir().unwrap();
    let rocks = crate::RealRocksDBStorage::new(dir.path()).await.unwrap();
    timed(&rocks, "RocksDB").await;
    #[cfg(feature = "surrealdb-storage")]
    {
        let surreal = crate::SurrealDBStorage::new("mem://", None, None, None, None)
            .await
            .unwrap();
        timed(&surreal, "SurrealDB (mem)").await;
    }
}
