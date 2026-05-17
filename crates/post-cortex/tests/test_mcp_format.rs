#![allow(missing_docs)]
use anyhow::Result;
use post_cortex::core::context_update::{ContextUpdate, UpdateContent, UpdateType};
use post_cortex_memory::{ConversationMemorySystem, SystemConfig};
use serial_test::serial;
use std::sync::Arc;
use tempfile::tempdir;
use uuid::Uuid;

#[serial]
#[tokio::test]
async fn test_mcp_format_semantic_search() -> Result<()> {
    // Test that mimics EXACTLY how MCP tool adds data and searches

    let temp_dir = tempdir()?;
    let mut config = SystemConfig::default();
    config.data_directory = temp_dir.path().to_str().unwrap().to_string();

    let system = Arc::new(
        ConversationMemorySystem::new(config)
            .await
            .map_err(|e| anyhow::anyhow!(e))?,
    );

    // Create session
    let session_id = system
        .create_session(
            Some("mcp-test".to_string()),
            Some("MCP format test".to_string()),
        )
        .await
        .map_err(|e| anyhow::anyhow!(e))?;

    // Create ContextUpdate EXACTLY like MCP tool does
    let question = "How does the lock-free conversation memory system work in Post-Cortex?";
    let answer = "The lock-free conversation memory system in Post-Cortex is built using Rust's advanced concurrency primitives to achieve zero-deadlock guarantees. It uses DashMap for concurrent hash maps, AtomicU64 for counters, ArcSwap for atomic pointer swapping, and an actor pattern for asynchronous RocksDB storage operations. The system implements a three-tier memory hierarchy with hot cache for 50 most recent items, warm cache for 200 items, and cold storage in RocksDB for everything else. Entity relationships are tracked using Petgraph, and semantic search is powered by local BERT models generating 384-dimensional L2-normalized embeddings stored in an HNSW index for fast nearest neighbor search. The entire architecture is designed to be production-grade with high throughput and low latency even under heavy concurrent load.";

    let update = ContextUpdate {
        id: Uuid::new_v4(),
        update_type: UpdateType::QuestionAnswered,
        content: UpdateContent {
            title: question.to_string(),     // Question as title
            description: answer.to_string(), // Answer as description
            details: vec![],
            examples: vec![],
            implications: vec![],
        },
        timestamp: chrono::Utc::now(),
        related_code: None,
        parent_update: None,
        user_marked_important: false,
        creates_entities: vec![],
        creates_relationships: vec![],
        references_entities: vec![],
        typed_entities: vec![],
    };

    // Add with metadata (like MCP tool does)
    let text = format!("{}\n{}", update.content.title, update.content.description);
    let metadata = Some(serde_json::to_value(&update)?);

    println!("Adding update with MCP format:");
    println!("  title: {}", &question[..50]);
    println!("  description: {}", &answer[..50]);
    println!("  combined text length: {}", text.len());

    system
        .add_incremental_update(session_id, text, metadata)
        .await
        .map_err(|e| anyhow::anyhow!(e))?;

    // Wait for vectorization
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    // Search with just the question (like user does)
    println!("\nSearching with query: {}", question);
    let results = system
        .semantic_search_session(session_id, question, Some(5), None, None)
        .await
        .map_err(|e| anyhow::anyhow!(e))?;

    println!("\nResults:");
    println!("Found {} results", results.len());

    if !results.is_empty() {
        let top_result = &results[0];
        println!(
            "Top result similarity: {:.2}%",
            top_result.similarity_score * 100.0
        );
        println!(
            "Top result combined score: {:.2}%",
            top_result.combined_score * 100.0
        );

        // This should be HIGH (>90%) because we're searching with the exact question
        // and the document contains both question (title) and answer (description)
        assert!(
            top_result.similarity_score > 0.90,
            "MCP format search should return high similarity (>90%) when searching with question. Got {:.2}%",
            top_result.similarity_score * 100.0
        );

        println!(
            "MCP format test PASSED - similarity is {:.2}%",
            top_result.similarity_score * 100.0
        );
    } else {
        panic!("No results found! Vectorization or search failed.");
    }

    Ok(())
}
