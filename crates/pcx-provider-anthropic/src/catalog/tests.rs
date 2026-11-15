// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use pcx_module_api::model::check_info;
use serde_json::json;

use super::*;

#[test]
fn its_table_passes_the_check() {
    let models = table().listing(None);
    assert!(models.iter().all(|m| check_info(m).is_ok()));
    let opus = models.iter().find(|m| m.id == "claude-opus-5-5").unwrap();
    assert_eq!(
        opus.price,
        Some(ModelPrice {
            input: 4_000_000,
            output: 20_000_000,
            cached_input: Some(200_000)
        })
    );
}

#[test]
fn a_listed_model_by_its_capabilities() {
    // An entry as `GET /v1/models` gives it.
    let entry = json!({
        "id": "claude-haiku-4-5", "display_name": "Claude Haiku 4.5",
        "max_input_tokens": 200_000, "max_tokens": 64_000,
        "capabilities": {
            "image_input": {"supported": true},
            "pdf_input": {"supported": false},
            "structured_outputs": {"supported": true},
            "thinking": {"supported": true, "types": {
                "enabled": {"supported": true}, "adaptive": {"supported": false}
            }}
        }
    });
    let m = model(&entry).unwrap();
    assert_eq!(
        (m.name.as_str(), m.context_tokens, m.output_tokens),
        ("Claude Haiku 4.5", 200_000, 64_000)
    );
    assert_eq!(m.input, modalities(&[Modality::Text, Modality::Image]));
    assert!(m.features.contains(&ModelFeature::Reasoning.into()));
    assert!(check_info(&m).is_ok());
    // It thinks, but not adaptively: it is not asked to.
    assert!(!adaptive(&entry));
    assert!(model(&json!({"display_name": "no id"})).is_none());
}
