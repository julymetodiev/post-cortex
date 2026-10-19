// Copyright (c) 2025 Julius ML
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

//! Formatting helpers for MCP tool results

use post_cortex_memory::content_vectorizer::SemanticSearchResult;
use serde_json::Value;

/// Format semantic search results as JSON array
///
/// Converts search results into a standardized JSON format with the following fields:
/// - `content`: The text content of the result
/// - `score`: The combined similarity/importance score
/// - `session_id`: The UUID of the session containing the result
/// - `type`: The content type (e.g., "qa", "decision_made")
/// - `timestamp`: RFC3339 timestamp of when the content was created
///
/// # Arguments
///
/// * `results` - Slice of semantic search results to format
///
/// # Returns
///
/// Vector of JSON values representing each result
///
/// # Example
///
/// ```rust,ignore
/// use post_cortex::daemon::format_helpers::format_search_results;
///
/// let formatted = format_search_results(&results);
/// // Returns: [{"text_content": "...", "similarity_score": 0.85, "combined_score": 0.9, ...}, ...]
/// ```
pub fn format_search_results(results: &[SemanticSearchResult]) -> Vec<Value> {
    results
        .iter()
        .map(|r| {
            // Same fields as session and global scope (post_cortex_mcp search
            // responses), so one JSONPath `select` works for every scope.
            let mut obj = serde_json::json!({
                "content_id": r.content_id,
                "session_id": r.session_id,
                "content_type": format!("{:?}", r.content_type),
                "text_content": r.text_content,
                "similarity_score": r.similarity_score,
                "importance_score": r.importance_score,
                "timestamp": r.timestamp.to_rfc3339(),
                "combined_score": r.combined_score
            });
            if let (Some(hit), Some(map)) = (&r.neighbour_of, obj.as_object_mut()) {
                map.insert("neighbour_of".to_string(), Value::String(hit.clone()));
            }
            obj
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use post_cortex_memory::content_vectorizer::ContentType;

    #[test]
    fn workspace_results_use_the_session_and_global_field_names() {
        let r = SemanticSearchResult {
            content_id: "c1".into(),
            session_id: uuid::Uuid::nil(),
            content_type: ContentType::DecisionPoint,
            text_content: "chose Rust".into(),
            similarity_score: 0.8,
            importance_score: 1.0,
            timestamp: chrono::Utc::now(),
            combined_score: 0.86,
            neighbour_of: None,
        };
        let v = &format_search_results(&[r])[0];
        for key in [
            "content_id",
            "session_id",
            "content_type",
            "text_content",
            "similarity_score",
            "importance_score",
            "combined_score",
            "timestamp",
        ] {
            assert!(v.get(key).is_some(), "missing {key}");
        }
        assert!(v.get("content").is_none() && v.get("score").is_none());
    }
}
