// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The source files a record names, as the files views read them: its code
//! reference, and for a code change the files its title lists (the `file`
//! of the write, free text that agents fill with several paths, brace
//! lists and the symbols they touched). Read time only: every stored
//! record counts as it is.

use post_cortex_core::core::context_update::{ContextUpdate, UpdateType};

/// Longest file extension taken for one (`.rs`, `.dart`, `.proto`,
/// `.gradle`); `Rerank.fallback` is an expression, not a file.
const MAX_EXTENSION: usize = 6;

/// Files known without an extension.
const BARE_FILES: &[&str] = &[
    "Makefile",
    "Dockerfile",
    "Justfile",
    "Procfile",
    "LICENSE",
    "Gemfile",
    "Rakefile",
];

/// The paths `update` names, each once, in the order they appear.
pub fn files_of(update: &ContextUpdate) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut add = |text: &str| {
        for path in paths_in(text) {
            if !out.contains(&path) {
                out.push(path);
            }
        }
    };
    if let Some(code) = &update.related_code {
        add(&code.file_path);
    }
    if update.update_type == UpdateType::CodeChanged {
        add(&update.content.title);
    }
    out
}

/// The path-like items of `text`: split on `,`, `;` and new lines outside
/// braces, `{a, b}` expanded against what surrounds it, each item cut at
/// its first space (a symbol or a note follows) and at a trailing `:line`.
/// A bare file name is a sibling of the path before it
/// (`src/a.rs, b.rs` names `src/b.rs`), and a leading `base:` is the
/// directory of the relative paths after it (`app: lib/a.dart`).
pub fn paths_in(text: &str) -> Vec<String> {
    let (base, text) = leading_base(text);
    let mut out: Vec<String> = Vec::new();
    for item in split_outside_braces(text) {
        for expanded in expand_braces(item.trim()) {
            let Some(path) = clean(&expanded) else {
                continue;
            };
            let bare = !path.contains(['/', '\\']);
            let path = match out.last().and_then(|p| p.rfind(['/', '\\'])) {
                Some(cut) if bare => format!("{}{path}", &out[out.len() - 1][..=cut]),
                _ if base.is_empty() || path.starts_with(['/', '\\']) => path,
                _ => format!("{base}/{path}"),
            };
            out.push(path);
        }
    }
    out
}

/// A leading `base: ` (one word ending in a colon, not a URL) and the rest
/// of `text`; an empty base when there is none.
fn leading_base(text: &str) -> (&str, &str) {
    let text = text.trim_start();
    match text.split_once(char::is_whitespace) {
        Some((word, rest))
            if word.len() > 1
                && word.ends_with(':')
                && !word.contains("://")
                && !word[..word.len() - 1].contains(':') =>
        {
            (word[..word.len() - 1].trim_end_matches(['/', '\\']), rest)
        }
        _ => ("", text),
    }
}

/// `text` split at `,`, `;` and new lines that are not inside braces.
fn split_outside_braces(text: &str) -> Vec<&str> {
    let mut items = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    for (i, c) in text.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            ',' | ';' | '\n' if depth == 0 => {
                items.push(&text[start..i]);
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    items.push(&text[start..]);
    items
}

/// `pre{a, b}post` as `prea post`, `preb post`, the alternatives expanded
/// in turn; text without a balanced brace pair stays as it is.
fn expand_braces(item: &str) -> Vec<String> {
    let Some(open) = item.find('{') else {
        return vec![item.to_string()];
    };
    let mut depth = 0usize;
    let mut close = None;
    for (i, c) in item[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(open + i);
                    break;
                }
            }
            _ => {}
        }
    }
    let Some(close) = close else {
        return vec![item.to_string()];
    };
    let (pre, inner, post) = (&item[..open], &item[open + 1..close], &item[close + 1..]);
    split_outside_braces(inner)
        .into_iter()
        .flat_map(|alt| expand_braces(&format!("{pre}{}{post}", first_word(alt.trim()))))
        .collect()
}

/// `text` up to its first whitespace.
fn first_word(text: &str) -> &str {
    text.split_whitespace().next().unwrap_or_default()
}

/// The path at the start of `item`, if it is one: cut at the first space,
/// without a trailing `:line[-end]` or punctuation; its file name has an
/// extension (or is a known file without one), and it has no character a
/// path would not have.
fn clean(item: &str) -> Option<String> {
    let word = first_word(item)
        .trim_start_matches(['(', '`', '"', '\''])
        .trim_end_matches([')', '`', '"', '\'', '.', ':']);
    let word = match word.rsplit_once(':') {
        Some((path, line))
            if !path.is_empty() && line.chars().all(|c| c.is_ascii_digit() || c == '-') =>
        {
            path
        }
        _ => word,
    };
    let name = word.rsplit(['/', '\\']).next().unwrap_or_default();
    // `pcx.views.v1` is a package, not a file with the extension `v1`.
    let version = |ext: &str| {
        ext.strip_prefix('v')
            .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
    };
    let has_extension = name.rsplit_once('.').is_some_and(|(stem, ext)| {
        !version(ext)
            && !stem.is_empty()
            && !ext.is_empty()
            && ext.len() <= MAX_EXTENSION
            && ext.chars().all(|c| c.is_ascii_alphanumeric())
    });
    let path_chars = word.chars().all(|c| {
        c.is_alphanumeric() || matches!(c, '/' | '\\' | '.' | '_' | '-' | '+' | '@' | '~' | ':')
    });
    (path_chars && (has_extension || BARE_FILES.contains(&name)) && !word.contains("://"))
        .then(|| word.to_string())
}
