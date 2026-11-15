// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! Arguments of the `get_structured_summary` tool. The schemars attributes are the tool's
//! published input schema, pinned by the daemon's `tools_list.json`.

use schemars::JsonSchema;
use serde::Deserialize;

/// Request payload for the `get_structured_summary` tool.
#[derive(Deserialize, JsonSchema, Debug)]
pub struct GetStructuredSummaryRequest {
    /// Session UUID to summarize.
    #[schemars(description = "Session ID")]
    pub session_id: String,
    /// Sections to include (`decisions`, `insights`, `entities`, `questions`, `all`).
    #[schemars(
        description = "Sections to include: decisions, insights, entities, questions, all (default: all)"
    )]
    pub include: Option<Vec<String>>,
    /// Maximum number of decisions to include.
    #[schemars(description = "Maximum decisions to include")]
    pub decisions_limit: Option<usize>,
    /// Maximum number of entities to include.
    #[schemars(description = "Maximum entities to include")]
    pub entities_limit: Option<usize>,
    /// Maximum number of questions to include.
    #[schemars(description = "Maximum questions to include")]
    pub questions_limit: Option<usize>,
    /// Maximum number of concepts to include.
    #[schemars(description = "Maximum concepts to include")]
    pub concepts_limit: Option<usize>,
    /// Minimum confidence level for included items.
    #[schemars(description = "Minimum confidence level")]
    pub min_confidence: Option<f32>,
    /// Whether to use compact output format.
    #[schemars(description = "Use compact format for large sessions")]
    pub compact: Option<bool>,
}
