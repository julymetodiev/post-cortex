// Copyright (c) 2025 Julius ML
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.

use super::config::{EmbeddingConfig, EmbeddingModelType};

#[test]
fn test_embedding_model_types() {
    assert_eq!(EmbeddingModelType::MiniLM.embedding_dimension(), 384);
    assert_eq!(
        EmbeddingModelType::MultilingualMiniLM.embedding_dimension(),
        384
    );
    assert_eq!(EmbeddingModelType::TinyBERT.embedding_dimension(), 312);
    assert_eq!(EmbeddingModelType::BGESmall.embedding_dimension(), 384);
    assert_eq!(
        EmbeddingModelType::PotionMultilingual.embedding_dimension(),
        256
    );
    assert_eq!(EmbeddingModelType::PotionCode.embedding_dimension(), 256);

    assert!(EmbeddingModelType::PotionMultilingual.is_model2vec());
    assert!(EmbeddingModelType::PotionCode.is_model2vec());
    assert!(!EmbeddingModelType::MultilingualMiniLM.is_model2vec());
}

/// Live load + encode of `potion-multilingual-128M` from HF Hub. Marked
/// `#[ignore]` because it needs network and downloads a ~50MB checkpoint.
/// Run with `cargo test -p post-cortex-embeddings model2vec_live -- --ignored`.
#[cfg(feature = "model2vec")]
#[tokio::test]
#[ignore = "requires network + HF Hub download (~50MB)"]
async fn test_potion_multilingual_live_load_and_encode() {
    use super::backends::Model2VecBackend;

    let backend = Model2VecBackend::load(EmbeddingModelType::PotionMultilingual)
        .await
        .expect("potion-multilingual-128M must load from HF Hub");

    use crate::embeddings::backend::EmbeddingBackend;
    assert_eq!(backend.embedding_dimension(), 256);

    let texts = vec![
        "Hello, world!".to_string(),
        "Здравей, свят!".to_string(), // Cyrillic — multilingual sanity check
        "こんにちは世界".to_string(), // CJK
    ];
    let embeddings = backend.embed(&texts).expect("encode must succeed");
    assert_eq!(embeddings.len(), texts.len());
    for v in &embeddings {
        assert_eq!(v.len(), 256, "every vector must match dim 256");
        // Vectors should be L2-normalised (potion default).
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(
            (norm - 1.0).abs() < 0.01,
            "potion outputs should be L2-normalised, got norm {}",
            norm
        );
    }
}

/// Live load + encode of `all-MiniLM-L6-v2` through Candle (the `bert`
/// backend), so a Candle upgrade is checked where it runs. Marked
/// `#[ignore]` because it needs network and downloads a ~90MB checkpoint.
/// Run with `scripts/cargo.sh ignored bert_live`.
#[cfg(feature = "bert")]
#[tokio::test]
#[ignore = "requires network + HF Hub download (~90MB)"]
async fn test_bert_live_load_and_encode() {
    use super::backends::BertBackend;
    use crate::embeddings::backend::EmbeddingBackend;

    let backend = BertBackend::load(EmbeddingModelType::MiniLM)
        .await
        .expect("all-MiniLM-L6-v2 must load from HF Hub");
    assert_eq!(backend.embedding_dimension(), 384);

    let texts = vec![
        "The cat sits on the mat.".to_string(),
        "A cat is sitting on a rug.".to_string(),
        "The quarterly tax report is due on Friday.".to_string(),
    ];
    let embeddings = backend.embed(&texts).expect("encode must succeed");
    assert_eq!(embeddings.len(), texts.len());
    for v in &embeddings {
        assert_eq!(v.len(), 384);
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 0.01, "L2-normalised, got norm {norm}");
    }
    let cosine = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>();
    let near = cosine(&embeddings[0], &embeddings[1]);
    let far = cosine(&embeddings[0], &embeddings[2]);
    assert!(
        near > 0.6 && far < 0.3,
        "paraphrases are near ({near}), unrelated text far ({far})"
    );
}

#[test]
fn test_potion_model_ids() {
    assert_eq!(
        EmbeddingModelType::PotionMultilingual.model_id(),
        "minishlab/potion-multilingual-128M"
    );
    assert_eq!(
        EmbeddingModelType::PotionCode.model_id(),
        "minishlab/potion-code-16M"
    );
}

#[test]
fn test_default_config() {
    let config = EmbeddingConfig::default();

    // Default flipped from MultilingualMiniLM (BERT) to PotionMultilingual
    // (Model2Vec) — smaller, faster, multilingual out of the box.
    assert_eq!(config.model_type, EmbeddingModelType::PotionMultilingual);
    assert_eq!(config.operation_timeout_secs, 30);
}
