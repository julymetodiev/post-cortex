// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use serde_json::json;

use super::*;

#[test]
fn a_listed_model_with_its_modalities_window_features_and_price() {
    // An entry as OpenRouter's /api/v1/models gives it (2026-10-31).
    let listing = json!({"data": [{
        "id": "anthropic/claude-sonnet-5.5",
        "name": "Anthropic: Claude Sonnet 5.5",
        "context_length": 1_000_000,
        "architecture": {
            "modality": "text+image+file->text",
            "input_modalities": ["text", "image", "file"],
            "output_modalities": ["text"]
        },
        "pricing": {"prompt": "0.000002", "completion": "0.00001", "input_cache_read": "0.0000001"},
        "top_provider": {"context_length": 1_000_000, "max_completion_tokens": 128_000},
        "supported_parameters": ["max_tokens", "reasoning", "tools", "structured_outputs"]
    }]});
    let models = served(&listing);
    assert_eq!(models.len(), 1);
    let m = &models[0];
    assert_eq!(m.id, "anthropic/claude-sonnet-5.5");
    assert_eq!((m.context_tokens, m.output_tokens), (1_000_000, 128_000));
    assert_eq!(m.kind, i32::from(ModelKind::Chat));
    let modalities = |ms: &[Modality]| ms.iter().map(|&m| i32::from(m)).collect::<Vec<_>>();
    assert_eq!(
        m.input,
        modalities(&[Modality::Text, Modality::Image, Modality::File])
    );
    assert_eq!(m.output, modalities(&[Modality::Text]));
    let features: Vec<i32> = [
        ModelFeature::Reasoning,
        ModelFeature::Tools,
        ModelFeature::StructuredOutput,
        ModelFeature::PromptCache,
    ]
    .map(i32::from)
    .to_vec();
    assert_eq!(m.features, features);
    // $2 and $10 a million tokens; cached $0.10.
    assert_eq!(
        m.price,
        Some(ModelPrice {
            input: 2_000_000,
            output: 10_000_000,
            cached_input: Some(100_000)
        })
    );
    assert!(pcx_module_api::model::check_info(m).is_ok());
}

#[test]
fn an_image_model_a_varying_price_and_unknown_modalities() {
    let listing = json!({"data": [
        {"id": "img", "architecture": {"input_modalities": ["text"], "output_modalities": ["image"]},
         "pricing": {"prompt": "-1", "completion": "-1"}},
        {"id": "odd", "architecture": {"input_modalities": ["smell"], "output_modalities": ["text"]}},
        {"name": "no id"}
    ]});
    let models = served(&listing);
    let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["img"]);
    assert_eq!(models[0].kind, i32::from(ModelKind::Image));
    assert_eq!(models[0].price, None);
    assert_eq!(models[0].context_tokens, 0);
}

/// OpenRouter's listing as it is today: every model it reads passes the
/// host's check, and most are read.
///
/// ```sh
/// cargo test -p pcx-provider-openrouter -- --ignored live_listing
/// ```
#[tokio::test]
#[ignore = "reads OpenRouter's public listing over the network"]
async fn live_listing_reads_and_passes_the_check() {
    let endpoint = pcx_openai_compat::Endpoint::new(ENDPOINT, "", codes()).unwrap();
    let listing = endpoint.models().await.unwrap();
    let all = listing["data"].as_array().unwrap().len();
    let models = served(&listing);
    assert!(
        models
            .iter()
            .all(|m| pcx_module_api::model::check_info(m).is_ok())
    );
    assert!(
        models.len() * 10 >= all * 9,
        "{} of {all} read",
        models.len()
    );
    println!("{} of {all} models read", models.len());
}
