// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

use super::*;

fn chat() -> ModelInfo {
    ModelInfo {
        id: "glm-5.3".to_string(),
        kind: ModelKind::Chat as i32,
        input: vec![Modality::Text as i32, Modality::Image as i32],
        output: vec![Modality::Text as i32],
        features: vec![ModelFeature::Tools as i32],
        ..ModelInfo::default()
    }
}

#[test]
fn a_model_in_shape_passes() {
    assert_eq!(check(&chat()), Ok(()));
}

#[test]
fn a_model_without_an_id_or_a_kind_is_refused() {
    let unnamed = ModelInfo {
        id: String::new(),
        ..chat()
    };
    assert_eq!(check(&unnamed), Err(ModelInfoError::NoId));
    for kind in [ModelKind::Unspecified as i32, 99] {
        let unkind = ModelInfo { kind, ..chat() };
        assert!(matches!(check(&unkind), Err(ModelInfoError::Kind { .. })));
    }
}

#[test]
fn a_model_must_take_and_give_something_known() {
    let deaf = ModelInfo {
        input: Vec::new(),
        ..chat()
    };
    assert!(matches!(
        check(&deaf),
        Err(ModelInfoError::NoModality { side: "input", .. })
    ));
    let odd = ModelInfo {
        output: vec![Modality::Unspecified as i32],
        ..chat()
    };
    assert!(matches!(
        check(&odd),
        Err(ModelInfoError::Unknown {
            field: "output",
            value: 0,
            ..
        })
    ));
    let feature = ModelInfo {
        features: vec![42],
        ..chat()
    };
    assert!(matches!(
        check(&feature),
        Err(ModelInfoError::Unknown {
            field: "features",
            value: 42,
            ..
        })
    ));
}
