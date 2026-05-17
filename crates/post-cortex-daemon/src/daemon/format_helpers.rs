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
/// ```rust
/// use post_cortex::daemon::format_helpers::format_search_results;
///
/// let formatted = format_search_results(&results);
/// // Returns: [{"content": "...", "score": 0.85, ...}, ...]
/// ```
pub fn format_search_results(results: &[SemanticSearchResult]) -> Vec<Value> {
    results
        .iter()
        .map(|r| {
            serde_json::json!({
                "content": r.text_content,
                "score": r.combined_score,
                "session_id": r.session_id,
                "type": format!("{:?}", r.content_type),
                "timestamp": r.timestamp.to_rfc3339()
            })
        })
        .collect()
}
