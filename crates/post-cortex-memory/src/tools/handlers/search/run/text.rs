// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The text block of a search reply: the part an agent reads.

/// " · ended 2026-10-20: superseded" for a record that no longer holds.
fn ended_note(r: &crate::content_vectorizer::SemanticSearchResult) -> String {
    let Some(v) = &r.validity else {
        return String::new();
    };
    let reason = v
        .reason
        .and_then(|reason| serde_json::to_value(reason).ok())
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "ended".to_string());
    match v.valid_to {
        Some(at) => format!(" · ended {}: {reason}", at.format("%Y-%m-%d")),
        None => String::new(),
    }
}

/// Build the text block returned alongside the JSON payload, the part an
/// agent reads. Results come as excerpts: each hit with the neighbours
/// returned for it (see `neighbour_of`), records in time order inside an
/// excerpt and excerpts oldest first, every record with its absolute time.
/// Matched records are marked `*`. `include_session` adds the session id to
/// each excerpt (global search).
pub fn results_message(
    results: &[crate::content_vectorizer::SemanticSearchResult],
    header: String,
    include_session: bool,
) -> String {
    use std::collections::HashMap;

    // Excerpt key: the hit's content id; a neighbour joins its hit's excerpt.
    let mut order: Vec<&str> = Vec::new();
    let mut excerpts: HashMap<&str, Vec<&crate::content_vectorizer::SemanticSearchResult>> =
        HashMap::new();
    for r in results {
        let key = r.neighbour_of.as_deref().unwrap_or(r.content_id.as_str());
        excerpts
            .entry(key)
            .or_insert_with(|| {
                order.push(key);
                Vec::new()
            })
            .push(r);
    }
    let mut excerpts: Vec<_> = order
        .into_iter()
        .filter_map(|k| excerpts.remove(k))
        .map(|mut records| {
            records.sort_by_key(|r| r.timestamp);
            records
        })
        .collect();
    excerpts.sort_by_key(|records| records[0].timestamp);

    let matches = results.iter().filter(|r| r.neighbour_of.is_none()).count();
    let mut message = format!(
        "{header} ({matches} matches, {} neighbouring records), oldest first:\n",
        results.len() - matches
    );
    for records in excerpts {
        message.push('\n');
        if include_session {
            let at = match records[0].source {
                crate::content_vectorizer::HitSource::Session(id) => format!("session {id}"),
                crate::content_vectorizer::HitSource::Workspace(id) => format!("workspace {id}"),
            };
            message.push_str(&format!("--- {at}\n"));
        } else {
            message.push_str("---\n");
        }
        for r in records {
            let mark = if r.neighbour_of.is_none() { "*" } else { " " };
            let by = r
                .author
                .as_deref()
                .map(|a| format!(" · {a}"))
                .unwrap_or_default();
            message.push_str(&format!(
                "{mark} {} [{:?}{by}{}] {}\n",
                r.timestamp.format("%Y-%m-%d %H:%M"),
                r.content_type,
                ended_note(r),
                r.text_content.trim()
            ));
        }
    }
    message
}
