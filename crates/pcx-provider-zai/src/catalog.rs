// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What Z.AI serves through this provider: the endpoint, the models and
//! their limits, and the error codes the agent acts on (Z.AI's API code
//! reference).

use pcx_module_api::model::{Modality, ModelFeature, ModelInfo, ModelKind, Table};
use pcx_openai_compat::Codes;

/// The coding plan's chat completions endpoint.
pub const ENDPOINT: &str = "https://api.z.ai/api/coding/paas/v4/chat/completions";

/// The models it knows: id, name, context and output limits. Each is a
/// chat model on text that reasons, calls tools and caches its prompt; the
/// coding plan has no price per token. The others Z.AI lists are unknown.
const MODELS: [(&str, &str, u64, u64); 1] = [("glm-5.3", "GLM-5.3", 1_048_576, 131_072)];

/// What it knows of Z.AI's models without its listing.
pub fn table() -> Table {
    Table { known: known() }
}

fn known() -> Vec<ModelInfo> {
    MODELS
        .iter()
        .map(|(id, name, context, output)| ModelInfo {
            id: (*id).to_string(),
            name: (*name).to_string(),
            context_tokens: *context,
            output_tokens: *output,
            kind: ModelKind::Chat as i32,
            input: vec![Modality::Text as i32],
            output: vec![Modality::Text as i32],
            features: vec![
                ModelFeature::Reasoning as i32,
                ModelFeature::Tools as i32,
                ModelFeature::PromptCache as i32,
            ],
            ..ModelInfo::default()
        })
        .collect()
}

fn strings(codes: &[&str]) -> Vec<String> {
    codes.iter().map(|c| (*c).to_string()).collect()
}

/// Z.AI's error codes: 1261 prompt too long, 1302 rate limit, 1305
/// overloaded, 1113 and 1308-1321 a balance, plan or window used up.
pub fn codes() -> Codes {
    Codes {
        context_too_long: strings(&["1261"]),
        rate_limited: strings(&["1302"]),
        overloaded: strings(&["1305"]),
        quota_exhausted: strings(&[
            "1113", "1308", "1309", "1310", "1311", "1313", "1314", "1315", "1316", "1317", "1318",
            "1319", "1320", "1321",
        ]),
    }
}
