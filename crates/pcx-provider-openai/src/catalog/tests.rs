// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use pcx_module_api::model::{check_info, unknown};
use serde_json::json;

use super::*;

#[test]
fn its_table_passes_the_check() {
    let models = table().listing(None);
    assert!(models.iter().all(|m| check_info(m).is_ok()));
    let terra = &models[1];
    assert_eq!(terra.id, "gpt-5.6-terra");
    assert_eq!(
        terra.price,
        Some(ModelPrice {
            input: 2_000_000,
            output: 12_000_000,
            cached_input: Some(200_000)
        })
    );
}

#[test]
fn a_listed_model_it_does_not_know_is_listed_unknown() {
    let listing = json!({"data": [{"id": "gpt-5.6-terra"}, {"id": "gpt-6"}]});
    let models = table().listing(Some(pcx_openai_compat::served(&listing)));
    assert_eq!(models[0].context_tokens, 1_050_000);
    assert_eq!(models[1], unknown("gpt-6"));
}

#[test]
fn a_used_up_quota_is_not_a_rate_limit() {
    let failure = codes().failure(
        Some(429),
        &json!({"code": "insufficient_quota", "message": "no"}),
    );
    assert_eq!(
        failure.kind,
        pcx_module_api::model::ModelFailureKind::QuotaExhausted
    );
}
