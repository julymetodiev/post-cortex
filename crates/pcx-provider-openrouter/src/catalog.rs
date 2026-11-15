// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What OpenRouter serves: its endpoint, its error codes, and its listing
//! (`GET /api/v1/models`) read into the models it routes.

use pcx_module_api::model::{Modality, ModelFeature, ModelInfo, ModelKind, ModelPrice};
use pcx_openai_compat::Codes;
use serde_json::Value;

/// Its chat completions endpoint.
pub const ENDPOINT: &str = "https://openrouter.ai/api/v1/chat/completions";

/// OpenRouter's error codes (HTTP-like numbers in its error object): 402
/// the credits used up.
pub fn codes() -> Codes {
    Codes {
        quota_exhausted: vec!["402".to_string()],
        ..Codes::default()
    }
}

/// The models of OpenRouter's `listing`, in its order; an entry without
/// an id, or that gives no modality it knows on either side, is left out.
pub fn served(listing: &Value) -> Vec<ModelInfo> {
    listing
        .get("data")
        .and_then(Value::as_array)
        .map(|data| data.iter().filter_map(model).collect())
        .unwrap_or_default()
}

fn model(entry: &Value) -> Option<ModelInfo> {
    let id = entry.get("id").and_then(Value::as_str)?;
    let architecture = entry.get("architecture");
    let side = |name: &str| -> Vec<i32> {
        architecture
            .and_then(|a| a.get(name))
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .filter_map(modality)
                    .map(i32::from)
                    .collect()
            })
            .unwrap_or_default()
    };
    let (input, output) = (side("input_modalities"), side("output_modalities"));
    if input.is_empty() || output.is_empty() {
        return None;
    }
    let pricing = entry.get("pricing");
    let parameters: Vec<&str> = entry
        .get("supported_parameters")
        .and_then(Value::as_array)
        .map(|p| p.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let cached = pricing.and_then(|p| per_million(p.get("input_cache_read")));
    let mut features = Vec::new();
    if parameters.contains(&"reasoning") {
        features.push(ModelFeature::Reasoning.into());
    }
    if parameters.contains(&"tools") {
        features.push(ModelFeature::Tools.into());
    }
    if parameters.contains(&"structured_outputs") {
        features.push(ModelFeature::StructuredOutput.into());
    }
    if cached.is_some() {
        features.push(ModelFeature::PromptCache.into());
    }
    let price = match (
        pricing.and_then(|p| per_million(p.get("prompt"))),
        pricing.and_then(|p| per_million(p.get("completion"))),
    ) {
        (Some(input), Some(output)) => Some(ModelPrice {
            input,
            output,
            cached_input: cached,
        }),
        _ => None,
    };
    let count = |value: Option<&Value>| value.and_then(Value::as_u64).unwrap_or(0);
    Some(ModelInfo {
        id: id.to_string(),
        name: entry
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        context_tokens: count(entry.get("context_length")),
        output_tokens: count(
            entry
                .get("top_provider")
                .and_then(|t| t.get("max_completion_tokens")),
        ),
        kind: kind(&output).into(),
        input,
        output,
        features,
        price,
        ..ModelInfo::default()
    })
}

/// A modality OpenRouter names.
fn modality(name: &str) -> Option<Modality> {
    match name {
        "text" => Some(Modality::Text),
        "image" => Some(Modality::Image),
        "audio" => Some(Modality::Audio),
        "video" => Some(Modality::Video),
        "file" => Some(Modality::File),
        _ => None,
    }
}

/// What a model that gives `output` is: one that writes text answers a
/// conversation; else what it makes.
fn kind(output: &[i32]) -> ModelKind {
    let gives = |m: Modality| output.contains(&i32::from(m));
    if gives(Modality::Text) {
        ModelKind::Chat
    } else if gives(Modality::Image) {
        ModelKind::Image
    } else if gives(Modality::Audio) {
        ModelKind::Speech
    } else {
        ModelKind::Video
    }
}

/// A price OpenRouter gives in US dollars a token (a string), in
/// micro-dollars a million tokens; a negative or unreadable one (a router
/// whose price varies) is not known.
fn per_million(value: Option<&Value>) -> Option<u64> {
    let dollars: f64 = value?.as_str()?.parse().ok()?;
    if !dollars.is_finite() || dollars < 0.0 {
        return None;
    }
    // Dollars a token = micro-dollars a million tokens, times 10^12 / 10^6.
    let micro = (dollars * 1e12).round();
    (micro <= u64::MAX as f64).then_some(micro as u64)
}

#[cfg(test)]
mod tests;
