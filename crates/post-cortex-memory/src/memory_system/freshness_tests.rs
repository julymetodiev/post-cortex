// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! File invalidation is scoped to one session: it removes that session's
//! records of the file and marks that session's source references stale,
//! never another session's, and it is not lost to a concurrent write. On
//! every backend.

use std::sync::Arc;

use post_cortex_core::core::context_update::CodeReference;
use post_cortex_core::core::freshness::{FreshnessStatus, SourceReference, SourceScope};
use uuid::Uuid;

use super::MemoryRuntime;
use super::backends_tests::{backends, request};
use crate::capabilities::Memory;

const FILE: &str = "src/shared.rs";

/// Write a record about `FILE` to `sid` and register its source.
async fn write_about_file(
    memory: &MemoryRuntime,
    service: &Memory,
    sid: Uuid,
    title: &str,
) -> Uuid {
    let mut req = request(sid, title, "touches the shared file");
    req.code_reference = Some(CodeReference {
        file_path: FILE.into(),
        start_line: 1,
        end_line: 2,
        code_snippet: "fn shared() {}".into(),
        commit_hash: None,
        branch: None,
        change_description: title.into(),
    });
    let entry_id = service.records.write(req).await.unwrap().entry_id;
    memory
        .stores
        .freshness
        .register_source(
            sid,
            SourceReference {
                entry_id: entry_id.to_string(),
                file_path: FILE.into(),
                content_hash: b"hash".to_vec(),
                captured_at_unix: 0,
                scope: SourceScope::WholeFile,
            },
        )
        .await
        .unwrap();
    entry_id
}

async fn status(memory: &MemoryRuntime, entry_id: Uuid) -> FreshnessStatus {
    let entries = memory
        .stores
        .freshness
        .check_freshness_batch(vec![(entry_id.to_string(), b"hash".to_vec(), None, None)])
        .await
        .unwrap();
    entries[0].status
}

async fn record_ids(memory: &MemoryRuntime, sid: Uuid) -> Vec<Uuid> {
    let session = memory.get_session(sid).await.unwrap().load_full();
    session.records().iter().map(|u| u.id).collect()
}

#[tokio::test]
async fn invalidating_a_file_for_one_session_leaves_other_sessions_alone() {
    for backend in backends().await {
        let memory = backend.start().await;
        let service = crate::capabilities::test_support::memory(&memory.clone());
        let a = memory.mem.sessions.create(None, None).await.unwrap();
        let b = memory.mem.sessions.create(None, None).await.unwrap();
        let in_a = write_about_file(&memory, &service, a, "Change in A").await;
        let in_b = write_about_file(&memory, &service, b, "Change in B").await;

        let (invalidated, _) = memory.mem.freshness.invalidate(a, FILE).await.unwrap();

        assert_eq!(invalidated, 1, "{}: only A's reference", backend.name);
        assert_eq!(
            status(&memory, in_a).await,
            FreshnessStatus::Stale,
            "{}",
            backend.name
        );
        assert_eq!(
            status(&memory, in_b).await,
            FreshnessStatus::Fresh,
            "{}: B's reference",
            backend.name
        );
        assert!(
            !record_ids(&memory, a).await.contains(&in_a),
            "{}",
            backend.name
        );
        assert_eq!(record_ids(&memory, b).await, [in_b], "{}", backend.name);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn invalidation_racing_writes_still_removes_the_records() {
    for backend in backends().await {
        let memory = backend.start().await;
        let service = Arc::new(crate::capabilities::test_support::memory(&memory.clone()));
        let sid = memory.mem.sessions.create(None, None).await.unwrap();
        for round in 0..30 {
            let stale = write_about_file(&memory, &service, sid, &format!("Change {round}")).await;
            let writers: Vec<_> = (0..8)
                .map(|i| {
                    let service = Arc::clone(&service);
                    tokio::spawn(async move {
                        service
                            .records
                            .write(request(sid, &format!("Other {round}.{i}"), "unrelated"))
                            .await
                            .unwrap();
                    })
                })
                .collect();
            let result = memory.mem.freshness.invalidate(sid, FILE).await;
            for w in writers {
                w.await.unwrap();
            }
            // Either the records are gone or the call said it failed.
            if result.is_ok() {
                assert!(
                    !record_ids(&memory, sid).await.contains(&stale),
                    "{}: round {round}: invalidation reported success but kept the record",
                    backend.name
                );
            }
        }
    }
}
