use crate::common;
use anyhow::Result;
use post_cortex::core::context_update::UpdateType;
use post_cortex_memory::SystemConfig;
use post_cortex_memory::capabilities::SearchScope;
use serial_test::serial;
use std::sync::Arc;
use tempfile::tempdir;

#[serial]
#[tokio::test]
async fn test_mcp_format_semantic_search() -> Result<()> {
    // Test that mimics EXACTLY how MCP tool adds data and searches

    let temp_dir = tempdir()?;
    let config = SystemConfig::default();

    let system = Arc::new(common::open_memory(common::rocksdb(temp_dir.path()), config).await?);

    // Create session
    let session_id = system
        .sessions
        .create(
            Some("mcp-test".to_string()),
            Some("MCP format test".to_string()),
        )
        .await
        .map_err(|e| anyhow::anyhow!(e))?;

    // Create ContextUpdate EXACTLY like MCP tool does
    let question = "How does the lock-free conversation memory system work in Post-Cortex?";
    let answer = "The lock-free conversation memory system in Post-Cortex is built using Rust's advanced concurrency primitives to achieve zero-deadlock guarantees. It uses DashMap for concurrent hash maps, AtomicU64 for counters, ArcSwap for atomic pointer swapping, and an actor pattern for asynchronous RocksDB storage operations. The system implements a three-tier memory hierarchy with hot cache for 50 most recent items, warm cache for 200 items, and cold storage in RocksDB for everything else. Entity relationships are tracked using Petgraph, and semantic search is powered by local BERT models generating 384-dimensional L2-normalized embeddings stored in an HNSW index for fast nearest neighbor search. The entire architecture is designed to be production-grade with high throughput and low latency even under heavy concurrent load.";

    // The real update (question as title) and a control with the same answer
    // under a neutral title — what the index held when the title was dropped.
    let control_title = "Architecture notes";
    for title in [question, control_title] {
        let update = common::record(session_id, UpdateType::QuestionAnswered, title, answer);
        // Add with metadata (like MCP tool does)
        let text = format!("{}\n{}", update.content.title, update.content.description);

        println!("Adding update with MCP format:");
        println!("  title: {}", &title[..title.len().min(50)]);
        println!("  combined text length: {}", text.len());

        // The write path of a record: stored (graph merged), then embedded
        // as the pipeline's embedding worker does.
        common::write(&system.records, update).await?;
        assert!(
            system
                .records
                .wait_idle(std::time::Duration::from_secs(120))
                .await
        );
    }

    // Wait for vectorization
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    // Search with just the question (like user does)
    println!("\nSearching with query: {}", question);
    let results = system
        .search
        .search(
            SearchScope::Session(session_id),
            question,
            post_cortex_memory::content_vectorizer::SearchOptions {
                limit: Some(5),
                ..Default::default()
            },
        )
        .await
        .map_err(|e| anyhow::anyhow!(e))?;

    println!("\nResults:");
    println!("Found {} results", results.len());

    if !results.is_empty() {
        let full = results
            .iter()
            .find(|r| r.text_content.contains(question))
            .expect("the update titled with the question must be found");
        let control = results
            .iter()
            .find(|r| r.text_content.contains(control_title));
        println!(
            "question-titled similarity: {:.2}%",
            full.similarity_score * 100.0
        );
        if let Some(c) = control {
            println!("control similarity: {:.2}%", c.similarity_score * 100.0);
        }

        // Absolute scores depend on the embedding model (BERT ~0.9, Potion
        // ~0.55), so check what the title is for: searching with the exact
        // question must rank the update that carries it first, clearly above
        // the same answer without it.
        assert!(
            results[0].text_content.contains(question),
            "question query must rank the question-titled update first; the title is not vectorized"
        );
        if let Some(c) = control {
            assert!(
                full.similarity_score > c.similarity_score + 0.05,
                "question-titled ({:.2}%) must beat the control ({:.2}%)",
                full.similarity_score * 100.0,
                c.similarity_score * 100.0
            );
        }

        println!(
            "MCP format test PASSED - similarity is {:.2}%",
            full.similarity_score * 100.0
        );
    } else {
        panic!("No results found! Vectorization or search failed.");
    }

    Ok(())
}
