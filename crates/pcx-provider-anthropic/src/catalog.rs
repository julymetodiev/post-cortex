// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What Anthropic serves through this provider: its API and version, the
//! models it knows (with their prices, which its listing does not give),
//! and its listing read into models.

use pcx_module_api::model::{Modality, ModelFeature, ModelInfo, ModelKind, ModelPrice, Table};
use serde_json::Value;

/// Its API.
pub const API: &str = "https://api.anthropic.com";

/// The API version it speaks.
pub const VERSION: &str = "2026-10-21";

/// The models it knows (2026-10-31): id, name, and the price in US
/// dollars a million tokens of input, output and cached input. Each has a
/// 1M window and 128K of output, takes text, images and PDF, thinks
/// adaptively, calls tools, answers in a schema and caches its prompt.
const MODELS: [(&str, &str, [f64; 3]); 7] = [
    ("claude-fable-5-1", "Claude Fable 5.1", [10.0, 50.0, 0.25]),
    ("claude-opus-5-5", "Claude Opus 5.5", [4.0, 20.0, 0.2]),
    ("claude-sonnet-5-5", "Claude Sonnet 5.5", [2.0, 10.0, 0.2]),
    ("claude-haiku-5-5", "Claude Haiku 5.5", [0.1, 0.5, 0.01]),
    ("claude-opus-5", "Claude Opus 5", [5.0, 25.0, 0.5]),
    ("claude-sonnet-5", "Claude Sonnet 5", [2.0, 10.0, 0.2]),
    ("claude-opus-4-8", "Claude Opus 4.8", [5.0, 25.0, 0.5]),
];

/// Dollars in micro-dollars.
fn micro(dollars: f64) -> u64 {
    // The table's prices are small and positive.
    (dollars * 1e6).round() as u64
}

fn modalities(of: &[Modality]) -> Vec<i32> {
    of.iter().map(|&m| i32::from(m)).collect()
}

/// What it knows of Anthropic's models without its listing.
pub fn table() -> Table {
    let known = MODELS
        .iter()
        .map(|(id, name, [input, output, cached])| ModelInfo {
            id: (*id).to_string(),
            name: (*name).to_string(),
            context_tokens: 1_000_000,
            output_tokens: 128_000,
            kind: ModelKind::Chat.into(),
            input: modalities(&[Modality::Text, Modality::Image, Modality::File]),
            output: modalities(&[Modality::Text]),
            features: [
                ModelFeature::Reasoning,
                ModelFeature::Tools,
                ModelFeature::StructuredOutput,
                ModelFeature::PromptCache,
            ]
            .map(i32::from)
            .to_vec(),
            price: Some(ModelPrice {
                input: micro(*input),
                output: micro(*output),
                cached_input: Some(micro(*cached)),
            }),
            ..ModelInfo::default()
        })
        .collect();
    Table { known }
}

/// Whether the capability at `pointer` (a leaf of the listing's tree) is
/// supported.
fn supported(entry: &Value, pointer: &str) -> bool {
    entry
        .pointer(&format!("/capabilities{pointer}/supported"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// Whether a listed model thinks adaptively: only then is it asked to.
pub fn adaptive(entry: &Value) -> bool {
    supported(entry, "/thinking/types/adaptive")
}

/// The model of a listing's entry (`GET /v1/models`'s `data`); none
/// without an id.
pub fn model(entry: &Value) -> Option<ModelInfo> {
    let id = entry.get("id").and_then(Value::as_str)?;
    let count = |field: &str| entry.get(field).and_then(Value::as_u64).unwrap_or(0);
    let mut input = vec![Modality::Text];
    if supported(entry, "/image_input") {
        input.push(Modality::Image);
    }
    if supported(entry, "/pdf_input") {
        input.push(Modality::File);
    }
    let mut features = Vec::new();
    if supported(entry, "/thinking") {
        features.push(ModelFeature::Reasoning);
    }
    features.push(ModelFeature::Tools);
    if supported(entry, "/structured_outputs") {
        features.push(ModelFeature::StructuredOutput);
    }
    features.push(ModelFeature::PromptCache);
    Some(ModelInfo {
        id: id.to_string(),
        name: entry
            .get("display_name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        context_tokens: count("max_input_tokens"),
        output_tokens: count("max_tokens"),
        kind: ModelKind::Chat.into(),
        input: modalities(&input),
        output: modalities(&[Modality::Text]),
        features: features.into_iter().map(i32::from).collect(),
        ..ModelInfo::default()
    })
}

#[cfg(test)]
mod tests;
