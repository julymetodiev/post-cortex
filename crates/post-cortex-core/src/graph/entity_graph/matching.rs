// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Which of the graph's entities a text names.

use crate::graph::entity_graph::SimpleEntityGraph;
use std::collections::HashSet;

/// Names of the graph's entities that `text` mentions, best match first.
///
/// Two strategies:
/// 1. **Name in text**: the entity's name appears in the text
///    (case-insensitive). Ranked first, longer names first.
/// 2. **Token overlap**: the name split into words (`name_tokens`:
///    camelCase, PascalCase, acronyms, snake_case, kebab-case, paths) and
///    matched against the text's words, a word matching when one is a
///    prefix (three characters or more) of the other ("stream" matches
///    "streaming"). Names of one or two words need all of them, longer
///    names 40% (at least one).
pub fn entities_in(text: &str, graph: &SimpleEntityGraph) -> Vec<String> {
    let text_lower = text.to_lowercase();
    let text_words: HashSet<&str> = text_lower
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|t| t.len() >= 3)
        .collect();

    let mut found: Vec<(&str, usize, bool)> = Vec::new(); // (name, score, in text)
    // By name: the sort below is stable, so equal scores keep this order.
    for (_, entity) in graph.entities.sorted() {
        let name_lower = entity.name.to_lowercase();
        if name_lower.len() < 2 {
            continue;
        }
        if text_lower.contains(&name_lower) {
            found.push((&entity.name, name_lower.len() * 10, true));
            continue;
        }

        let tokens = name_tokens(&entity.name);
        let matched = tokens
            .iter()
            .filter(|token| token.len() >= 3 && text_words.iter().any(|w| prefix_match(token, w)))
            .count();
        if matched == 0 {
            continue;
        }
        let threshold = if tokens.len() <= 2 {
            tokens.len()
        } else {
            1.max((tokens.len() * 2).div_ceil(5))
        };
        if matched >= threshold {
            found.push((&entity.name, matched * 5 + name_lower.len(), false));
        }
    }

    found.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| b.1.cmp(&a.1)));
    found
        .into_iter()
        .map(|(name, _, _)| name.to_string())
        .collect()
}

/// One word is a prefix of the other, the shorter having three characters
/// or more.
fn prefix_match(a: &str, b: &str) -> bool {
    let (shorter, longer) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    shorter.len() >= 3 && longer.starts_with(shorter)
}

/// Lowercase words of an entity name. Splits on `_ - / . ` and whitespace,
/// and on case changes of the name as written, before lowercasing:
/// "ChatRepositoryImpl" is chat, repository, impl; "HTTPServer" is http,
/// server; "PG LISTEN/NOTIFY" is pg, listen, notify.
pub fn name_tokens(name: &str) -> Vec<String> {
    let chars: Vec<char> = name.chars().collect();
    let mut tokens = Vec::new();
    let mut current = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if matches!(c, '_' | '-' | '/' | '.') || c.is_whitespace() {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
            continue;
        }
        if c.is_uppercase() && !current.is_empty() {
            let prev = chars[i - 1];
            let next_is_lower = chars.get(i + 1).is_some_and(|n| n.is_lowercase());
            if prev.is_lowercase() || prev.is_numeric() || (prev.is_uppercase() && next_is_lower) {
                tokens.push(std::mem::take(&mut current));
            }
        }
        current.extend(c.to_lowercase());
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// The [`entity_key`](crate::graph::entity_graph::entity_key) of every run of up to `max_words`
/// consecutive words of `text`: the keys under which an entity whose name
/// has at most `max_words` words ([`name_words`]) would be named there.
/// Words keep the characters names use (`+#./-_'`), lose a trailing
/// possessive ("Caroline's" is "Caroline"), and match whole: "go" is not
/// in "good".
pub fn name_keys_in(text: &str, max_words: usize) -> HashSet<String> {
    let words: Vec<&str> = text
        .split(|c: char| !(c.is_alphanumeric() || "+#./-_'’".contains(c)))
        .map(|w| w.trim_matches(|c: char| matches!(c, '.' | '\'' | '’')))
        .map(|w| {
            w.strip_suffix("'s")
                .or_else(|| w.strip_suffix("’s"))
                .unwrap_or(w)
        })
        .filter(|w| !w.is_empty())
        .collect();
    let mut keys = HashSet::new();
    for n in 1..=max_words.min(words.len()) {
        for run in words.windows(n) {
            keys.insert(crate::graph::entity_graph::entity_key(&run.join(" ")));
        }
    }
    keys
}

/// Words of an entity name as [`name_keys_in`] counts them (separated by
/// whitespace, `-` or `_`, the characters the resolution key drops).
pub fn name_words(name: &str) -> usize {
    name.split(|c: char| c.is_whitespace() || c == '-' || c == '_')
        .filter(|w| !w.is_empty())
        .count()
}
