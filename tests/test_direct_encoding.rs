use anyhow::Result;
use post_cortex_embeddings::{EmbeddingConfig, LocalEmbeddingEngine};

#[tokio::test]
async fn test_direct_query_encoding_consistency() -> Result<()> {
    // Test that encoding the EXACT same query twice gives >95% similarity

    let config = EmbeddingConfig::default();
    let engine = LocalEmbeddingEngine::new(config).await?;

    // This is the EXACT query we're using in production
    let query = "How does the lock-free conversation memory system work in Post-Cortex?";

    // Encode it 5 times to check consistency
    let mut embeddings = Vec::new();
    for i in 0..5 {
        let emb = engine.encode_text(query).await?;
        println!("Encoding {}: first 5 dims = {:?}", i, &emb[0..5]);

        // Check norm
        let norm: f32 = emb.iter().map(|x| x * x).sum::<f32>().sqrt();
        println!("Encoding {} norm: {}", i, norm);

        embeddings.push(emb);
    }

    // Compare all pairs
    for i in 0..embeddings.len() {
        for j in (i + 1)..embeddings.len() {
            let emb1 = &embeddings[i];
            let emb2 = &embeddings[j];

            let dot_product: f32 = emb1.iter().zip(emb2.iter()).map(|(x, y)| x * y).sum();
            let norm1: f32 = emb1.iter().map(|x| x * x).sum::<f32>().sqrt();
            let norm2: f32 = emb2.iter().map(|x| x * x).sum::<f32>().sqrt();

            let similarity = if norm1 == 0.0 || norm2 == 0.0 {
                0.0
            } else {
                dot_product / (norm1 * norm2)
            };

            println!(
                "Similarity between encoding {} and {}: {:.4} ({:.2}%)",
                i,
                j,
                similarity,
                similarity * 100.0
            );

            assert!(
                similarity > 0.95,
                "Identical query should have >95% similarity, got {:.2}%",
                similarity * 100.0
            );
        }
    }

    Ok(())
}
