// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Search responses: one shape for every scope and every transport (it was
//! three in MCP and another in gRPC `select`).

use crate::content_vectorizer::{SemanticSearchResult, Signals};
use chrono::{DateTime, Utc};
use post_cortex_core::graph::temporal::{EndReason, Validity};
use serde::Serialize;
use uuid::Uuid;

use crate::tools::responses::rfc3339;

/// The hits of a search.
#[derive(Debug, Clone, Serialize)]
pub struct SearchResponse {
    /// The session searched (session scope).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// The workspace searched (workspace scope).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    /// The query.
    pub query: String,
    /// Hits returned.
    pub total_results: usize,
    /// The hits, best first.
    pub results: Vec<SearchHit>,
    /// Whether a reranker ordered them.
    pub reranked: bool,
    /// Why there are none, when that is known up front.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<&'static str>,
}

impl SearchResponse {
    /// The response of `hits` for `query`, `reranked` when a reranker
    /// ordered them.
    #[must_use]
    pub fn new(query: &str, hits: &[SemanticSearchResult], reranked: bool) -> Self {
        let results: Vec<SearchHit> = hits.iter().map(SearchHit::from).collect();
        Self {
            session_id: None,
            workspace_id: None,
            query: query.to_string(),
            total_results: results.len(),
            results,
            reranked,
            note: None,
        }
    }
}

/// One hit of a search.
#[derive(Debug, Clone, Serialize)]
pub struct SearchHit {
    /// The record id, `entity:<name>` or `message:<seq>`.
    pub content_id: String,
    /// What it is (stored name: `UpdateContent`, `EntityDescription`, ...).
    pub content_type: &'static str,
    /// Its text, or its passage for the query.
    pub text_content: String,
    /// The whole text's length in characters, when `text_content` is its
    /// passage.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub full_chars: Option<usize>,
    /// Vector similarity.
    pub similarity_score: f64,
    /// Importance.
    pub importance_score: f64,
    /// When it was written.
    pub timestamp: String,
    /// The ranking score.
    pub combined_score: f64,
    /// Its session (empty for a document).
    pub session_id: String,
    /// A document's workspace.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    /// A document's kind (`<module>/<name>`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document_kind: Option<String>,
    /// A document's version found.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document_version: Option<u64>,
    /// A record's or a document's title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The hit this neighbouring record was returned for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub neighbour_of: Option<String>,
    /// Who wrote it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// The captured messages it was drawn from.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub derived_from: Vec<String>,
    /// When it stopped holding, for a record that no longer does.
    #[serde(flatten)]
    pub ended: Option<Ended>,
    /// The writer's confidence.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
    /// Ranking signals (traced searches).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signals: Option<Signals>,
}

/// The end of a record's validity.
#[derive(Debug, Clone, Serialize)]
pub struct Ended {
    /// When it stopped holding.
    pub valid_to: Option<DateTime<Utc>>,
    /// The record that ended it.
    pub ended_by: Option<Uuid>,
    /// Why.
    pub end_reason: Option<EndReason>,
}

impl Ended {
    /// The end `validity` states.
    pub fn of(validity: &Validity) -> Self {
        Self {
            valid_to: validity.valid_to,
            ended_by: validity.ended_by,
            end_reason: validity.reason,
        }
    }
}

/// A score as a hit shows it: three decimals, as many as rank results
/// apart.
fn score(x: f32) -> f64 {
    (f64::from(x) * 1000.0).round() / 1000.0
}

impl From<&SemanticSearchResult> for SearchHit {
    fn from(r: &SemanticSearchResult) -> Self {
        Self {
            content_id: r.content_id.clone(),
            content_type: r.content_type.stored_name(),
            text_content: r.text_content.clone(),
            full_chars: r.full_chars,
            similarity_score: score(r.similarity_score),
            importance_score: score(r.importance_score),
            timestamp: rfc3339(r.timestamp),
            combined_score: score(r.combined_score),
            session_id: session_of(r),
            workspace_id: r.source.workspace().map(|id| id.to_string()),
            document_kind: r.document.as_ref().map(|d| d.kind.clone()),
            document_version: r.document.as_ref().map(|d| d.version),
            title: r.title.clone(),
            neighbour_of: r.neighbour_of.clone(),
            author: r.author.clone(),
            derived_from: r.derived_from.clone(),
            ended: r.validity.as_ref().map(Ended::of),
            confidence: r.confidence,
            signals: r.signals.clone(),
        }
    }
}

/// Content related to a topic in a session.
#[derive(Debug, Clone, Serialize)]
pub struct RelatedContent {
    /// The session.
    pub session_id: String,
    /// The topic.
    pub topic: String,
    /// The related items, best first.
    pub related_content: Vec<RelatedHit>,
}

/// One item of [`RelatedContent`].
#[derive(Debug, Clone, Serialize)]
pub struct RelatedHit {
    /// The record id, `entity:<name>` or `message:<seq>`.
    pub content_id: String,
    /// Its session (empty for a document).
    pub session_id: String,
    /// A document's workspace.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    /// What it is (stored name).
    pub content_type: &'static str,
    /// Its text.
    pub text_content: String,
    /// Vector similarity.
    pub similarity_score: f32,
    /// Importance.
    pub importance_score: f32,
    /// When it was written.
    pub timestamp: String,
    /// The ranking score.
    pub combined_score: f32,
}

impl From<&SemanticSearchResult> for RelatedHit {
    fn from(r: &SemanticSearchResult) -> Self {
        Self {
            content_id: r.content_id.clone(),
            session_id: session_of(r),
            workspace_id: r.source.workspace().map(|id| id.to_string()),
            content_type: r.content_type.stored_name(),
            text_content: r.text_content.clone(),
            similarity_score: r.similarity_score,
            importance_score: r.importance_score,
            timestamp: rfc3339(r.timestamp),
            combined_score: r.combined_score,
        }
    }
}

/// A hit's session as the wire carries it: empty for a document.
fn session_of(r: &SemanticSearchResult) -> String {
    r.source
        .session()
        .map(|id| id.to_string())
        .unwrap_or_default()
}
