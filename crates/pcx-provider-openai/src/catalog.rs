// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What OpenAI serves through this provider: the endpoint, the models it
//! knows (its listing gives only ids) and the error codes the agent acts
//! on.

use pcx_module_api::model::{Modality, ModelFeature, ModelInfo, ModelKind, ModelPrice, Table};
use pcx_openai_compat::Codes;

/// Its chat completions endpoint.
pub const ENDPOINT: &str = "https://api.openai.com/v1/chat/completions";

/// The models it knows (2026-10-31): id, name, context, output, and the
/// price in US dollars a million tokens of input, output and cached input.
/// Each is a chat model on text, images and files that reasons, calls
/// tools, answers in a schema and caches its prompt.
const MODELS: [(&str, &str, u64, u64, [f64; 3]); 7] = [
    (
        "gpt-5.6-sol",
        "GPT-5.6 Sol",
        1_050_000,
        128_000,
        [4.0, 20.0, 0.4],
    ),
    (
        "gpt-5.6-terra",
        "GPT-5.6 Terra",
        1_050_000,
        128_000,
        [2.0, 12.0, 0.2],
    ),
    (
        "gpt-5.6-luna",
        "GPT-5.6 Luna",
        1_050_000,
        128_000,
        [0.2, 1.2, 0.02],
    ),
    ("gpt-5.5", "GPT-5.5", 1_050_000, 128_000, [5.0, 30.0, 0.5]),
    ("gpt-5.4", "GPT-5.4", 1_050_000, 128_000, [2.5, 15.0, 0.25]),
    (
        "gpt-5.4-mini",
        "GPT-5.4 mini",
        400_000,
        128_000,
        [0.75, 4.5, 0.075],
    ),
    (
        "gpt-5.4-nano",
        "GPT-5.4 nano",
        400_000,
        128_000,
        [0.2, 1.25, 0.02],
    ),
];

/// Dollars in micro-dollars.
fn micro(dollars: f64) -> u64 {
    // The table's prices are small and positive.
    (dollars * 1e6).round() as u64
}

/// What it knows of OpenAI's models without its listing.
pub fn table() -> Table {
    let known = MODELS
        .iter()
        .map(
            |(id, name, context, output, [input, out, cached])| ModelInfo {
                id: (*id).to_string(),
                name: (*name).to_string(),
                context_tokens: *context,
                output_tokens: *output,
                kind: ModelKind::Chat.into(),
                input: [Modality::Text, Modality::Image, Modality::File]
                    .map(i32::from)
                    .to_vec(),
                output: vec![Modality::Text.into()],
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
                    output: micro(*out),
                    cached_input: Some(micro(*cached)),
                }),
                ..ModelInfo::default()
            },
        )
        .collect();
    Table { known }
}

fn strings(codes: &[&str]) -> Vec<String> {
    codes.iter().map(|c| (*c).to_string()).collect()
}

/// OpenAI's error codes.
pub fn codes() -> Codes {
    Codes {
        context_too_long: strings(&["context_length_exceeded"]),
        rate_limited: strings(&["rate_limit_exceeded"]),
        overloaded: strings(&["server_overloaded"]),
        quota_exhausted: strings(&["insufficient_quota"]),
    }
}

#[cfg(test)]
mod tests;
