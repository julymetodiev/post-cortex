// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Tokenizing text for lexical (BM25) search, shared by every backend: the
//! SurrealDB full-text query and the in-memory index of
//! [`VectorDB`](crate::VectorDB). Language-independent: no stopword lists.
//! Words too common to help are found from the data instead, by document
//! frequency ([`VectorDB::query_terms`](crate::VectorDB::query_terms)).

/// The words of `text`, in order and with repeats: lowercased, split on
/// anything that is not a letter or digit (any script).
pub fn tokens(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(String::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::tokens;

    #[test]
    fn splits_any_script_on_punctuation() {
        assert_eq!(
            tokens("What did Caroline say?").join(" "),
            "what did caroline say"
        );
        assert_eq!(
            tokens("Какво решихме за базата?").join(" "),
            "какво решихме за базата"
        );
        assert_eq!(
            tokens("Přišel jsem, viděl jsem").join(" "),
            "přišel jsem viděl jsem"
        );
        assert!(tokens("?!").is_empty());
    }
}
