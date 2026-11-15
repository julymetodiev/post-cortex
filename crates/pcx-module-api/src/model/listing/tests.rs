// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

fn known(id: &str, context: u64) -> ModelInfo {
    ModelInfo {
        name: id.to_uppercase(),
        context_tokens: context,
        ..unknown(id)
    }
}

fn table() -> Table {
    Table {
        known: vec![known("a", 100), known("b", 200)],
    }
}

fn ids(models: &[ModelInfo]) -> Vec<&str> {
    models.iter().map(|m| m.id.as_str()).collect()
}

#[test]
fn without_its_api_the_table() {
    let listed = table().listing(None);
    assert_eq!(ids(&listed), ["a", "b"]);
}

#[test]
fn the_api_names_the_models_and_the_table_describes_them() {
    let served = vec![unknown("b"), unknown("c")];
    let listed = table().listing(Some(served));
    // `a` is not served any more; `b` as the table knows it; `c` unknown.
    assert_eq!(ids(&listed), ["b", "c"]);
    assert_eq!(listed[0].context_tokens, 200);
    assert_eq!(listed[1].context_tokens, 0);
    assert_eq!(listed[1].kind, i32::from(ModelKind::Chat));
}

#[test]
fn an_unknown_model_passes_the_check() {
    assert!(crate::model::check_info(&unknown("x")).is_ok());
}

#[test]
fn a_refused_key_fails_and_an_api_down_leaves_the_table() {
    let refused = ModelFailure::new(ModelFailureKind::Unauthorized, "bad key");
    let failed = table().listed("p", Err(refused)).unwrap_err();
    assert_eq!(failed.kind, ModelFailureKind::Unauthorized);
    let down = ModelFailure::new(ModelFailureKind::Unavailable, "down");
    let listed = table().listed("p", Err(down)).unwrap();
    assert_eq!(ids(&listed), ["a", "b"]);
}
