// Copyright (c) 2026 Julius ML
// MIT License

//! The parity test of the transports (ARCHITECTURE rule 3, section 6):
//! every operation of the table is exposed by the transports the table
//! names and nothing else is exposed; the same request through MCP, gRPC
//! and REST returns the same response shape; a failure is the same kind on
//! every transport.

use std::collections::BTreeSet;
use std::sync::Arc;

use post_cortex_memory::tools::operations::{self, OPERATIONS};
use post_cortex_memory::{Memory, SystemConfig};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::daemon::test_client::{Client, policy, text_of};
use post_cortex_transport::memory::PcxGrpcService;

mod error_tests;
mod shape_tests;
mod snippet_tests;

/// A memory instance (no embeddings) with its MCP client, gRPC service and
/// HTTP router over one router.
struct Transports {
    memory: Arc<Memory>,
    mcp: Client,
    grpc: PcxGrpcService,
    router: axum::Router,
    ct: CancellationToken,
    _dir: tempfile::TempDir,
}

async fn transports() -> Transports {
    transports_with(false).await
}

/// [`transports`] with embeddings on or off.
async fn transports_with(enable_embeddings: bool) -> Transports {
    let dir = tempfile::tempdir().unwrap();
    let memory = Arc::new(
        crate::memory_tests::rocksdb_memory(
            &dir,
            SystemConfig {
                enable_embeddings,
                ..Default::default()
            },
        )
        .await
        .unwrap(),
    );
    let ct = CancellationToken::new();
    let router = crate::daemon::test_client::router(memory.clone(), &policy(), None, &ct).await;
    Transports {
        mcp: Client::connect(router.clone()).await,
        grpc: PcxGrpcService::new(
            memory.clone(),
            crate::compose::BUILD,
            post_cortex_memory::MemoryModule::new(memory.clone())
                .unwrap()
                .kinds(),
        ),
        memory,
        router,
        ct,
        _dir: dir,
    }
}

/// The data of an MCP reply (`Results:`) or of a failure (`Details:`).
fn mcp_data(message: &Value) -> Option<Value> {
    let text = text_of(message);
    ["Results:", "Details:"].iter().find_map(|marker| {
        let (_, json) = text.split_once(marker)?;
        serde_json::from_str(json.trim()).ok()
    })
}

/// Every RPC of `pcx.v1.PostCortex` is an operation of the table, and
/// every RPC the table names exists.
#[test]
fn grpc_exposes_exactly_the_table() {
    let proto = include_str!("../../../../post-cortex-proto/proto/pcx.proto");
    let served: BTreeSet<&str> = proto
        .lines()
        .filter_map(|l| l.trim().strip_prefix("rpc "))
        .filter_map(|l| l.split('(').next())
        .map(str::trim)
        .collect();
    let table: BTreeSet<&str> = operations::grpc_methods().into_iter().collect();
    assert_eq!(served, table);
}

/// Every RPC of `pcx.v1.Blobs` is an operation of the table, and every one
/// the table names exists.
#[test]
fn blobs_expose_exactly_the_table() {
    let proto = include_str!("../../../../post-cortex-proto/proto/pcx_blob.proto");
    let served: BTreeSet<&str> = proto
        .lines()
        .filter_map(|l| l.trim().strip_prefix("rpc "))
        .filter_map(|l| l.split('(').next())
        .map(str::trim)
        .collect();
    let table: BTreeSet<&str> = operations::blobs_methods().into_iter().collect();
    assert_eq!(served, table);
}

/// Every MCP tool the server lists is in the table, and every action or
/// query type the table routes reaches a handler (none answers that it is
/// an unknown action).
#[tokio::test]
async fn mcp_exposes_exactly_the_table() {
    let mut t = transports().await;
    let listed: BTreeSet<String> = t.mcp.list_tools().await["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap().to_string())
        .collect();
    let table: BTreeSet<String> = operations::mcp_tools()
        .into_iter()
        .map(String::from)
        .collect();
    assert_eq!(listed, table);

    let sid = t
        .memory
        .sessions
        .create(None, None)
        .await
        .unwrap()
        .to_string();
    for op in OPERATIONS {
        for route in op.mcp {
            let Some(selector) = operations::selector(route.tool) else {
                continue;
            };
            for value in route.values {
                let args = json!({selector: value, "session_id": sid, "parameters": {}});
                let message = t.mcp.call_raw(route.tool, args).await;
                let text = text_of(&message);
                assert!(message.get("error").is_none(), "{}: {message}", op.name);
                for unknown in ["Invalid action", "Unknown query type", "does not handle"] {
                    assert!(
                        !text.contains(unknown),
                        "{} {selector}={value} ({}) is not handled: {text}",
                        route.tool,
                        op.name
                    );
                }
            }
        }
    }
    // A value the table does not route is refused.
    let message = t
        .mcp
        .call_raw("session", json!({"action": "teleport"}))
        .await;
    assert!(text_of(&message).contains("Invalid action"));
    t.ct.cancel();
}
