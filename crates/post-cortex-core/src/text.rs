// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Tokenizing text for lexical (BM25) search, shared by every index: the
//! SurrealDB full-text query and the in-memory index of
//! `post-cortex-embeddings`. Language-independent: no stopword lists.
//! Words too common to help are found from the data instead, by document
//! frequency (`VectorDB::query_terms`). And the passage of a long text a
//! search shows for a query, found by the same words.

/// A word of a text: its byte range and its character range.
struct Word {
    bytes: (usize, usize),
    chars: (usize, usize),
}

/// The words of `text`, in order.
fn words(text: &str) -> Vec<Word> {
    let mut words = Vec::new();
    let mut start: Option<(usize, usize)> = None;
    let mut count = 0;
    for (at, c) in text.char_indices() {
        match (c.is_alphanumeric(), start) {
            (true, None) => start = Some((at, count)),
            (false, Some((byte, char))) => {
                words.push(Word {
                    bytes: (byte, at),
                    chars: (char, count),
                });
                start = None;
            }
            _ => {}
        }
        count += 1;
    }
    if let Some((byte, char)) = start {
        words.push(Word {
            bytes: (byte, text.len()),
            chars: (char, count),
        });
    }
    words
}

/// The words of `text`, in order and with repeats: lowercased, split on
/// anything that is not a letter or digit (any script).
pub fn tokens(text: &str) -> Vec<String> {
    words(text)
        .into_iter()
        .map(|w| text[w.bytes.0..w.bytes.1].to_lowercase())
        .collect()
}

/// The passage of `text` a search shows for `query`: none when `text` is at
/// most `max_chars` characters (it is shown whole); else about `max_chars`
/// characters around the words of the query, the window that holds the
/// most of them (the first when none does), cut at words and marked `…`
/// where it was cut.
#[must_use]
pub fn passage(text: &str, query: &str, max_chars: usize) -> Option<String> {
    if text.chars().count() <= max_chars {
        return None;
    }
    let mut terms: Vec<String> = tokens(query);
    terms.sort();
    terms.dedup();
    let words = words(text);
    if words.is_empty() {
        return Some(format!(
            "{}…",
            text.chars().take(max_chars).collect::<String>()
        ));
    }
    // Which query word each word of the text is, if any.
    let term: Vec<Option<usize>> = words
        .iter()
        .map(|w| {
            terms
                .binary_search(&text[w.bytes.0..w.bytes.1].to_lowercase())
                .ok()
        })
        .collect();
    // Slide a window of at most `max_chars` characters over the words,
    // counting the distinct query words it holds; the best window that
    // starts at a query word wins (its words are then led into).
    let mut held = vec![0usize; terms.len()];
    let (mut distinct, mut end) = (0usize, 0usize);
    let (mut best, mut best_distinct) = (0usize, 0usize);
    for first in 0..words.len() {
        while end < words.len() && words[end].chars.1 - words[first].chars.0 <= max_chars {
            if let Some(t) = term[end] {
                held[t] += 1;
                if held[t] == 1 {
                    distinct += 1;
                }
            }
            end += 1;
        }
        if term[first].is_some() && distinct > best_distinct {
            (best, best_distinct) = (first, distinct);
        }
        if end > first
            && let Some(t) = term[first]
        {
            held[t] -= 1;
            if held[t] == 0 {
                distinct -= 1;
            }
        }
    }
    // Lead in a little, so the first word found is not the passage's first.
    let lead = max_chars / 8;
    let mut first = best;
    if best_distinct > 0 {
        while first > 0 && words[best].chars.0 - words[first - 1].chars.0 <= lead {
            first -= 1;
        }
    }
    let from = words[first].chars.0;
    let last = words[first..]
        .iter()
        .take_while(|w| w.chars.1 - from <= max_chars)
        .last()
        .map_or(words[first].bytes.1, |w| w.bytes.1);
    let start = words[first].bytes.0;
    let mut shown = String::new();
    if start > 0 {
        shown.push('…');
    }
    shown.push_str(&text[start..last]);
    if text[last..].chars().any(char::is_alphanumeric) {
        shown.push('…');
    }
    Some(shown)
}

#[cfg(test)]
mod tests;
