// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! An OpenAI-compatible listing of models (`{"data": [{"id": …}]}`): the
//! ids it serves, each a model its provider's table describes where it
//! knows it (docs/modules/agent.md, choosing the models).

use pcx_module_api::model::{ModelInfo, unknown};
use serde_json::Value;

/// The models `listing` names, in its order, each as an unknown model.
#[must_use]
pub fn served(listing: &Value) -> Vec<ModelInfo> {
    listing
        .get("data")
        .and_then(Value::as_array)
        .map(|data| {
            data.iter()
                .filter_map(|m| m.get("id").and_then(Value::as_str))
                .map(unknown)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;
