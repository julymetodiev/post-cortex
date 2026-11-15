// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The pure part of the file tools: no I/O, text in, text out.

use pcx_module_api::{ModuleError, ModuleErrorKind};
use regex::Regex;

/// The text of a file as UTF-8, or `InvalidArgument` for a binary one.
pub fn utf8(path: &str, bytes: Vec<u8>) -> Result<String, ModuleError> {
    String::from_utf8(bytes).map_err(|_| {
        ModuleError::new(
            ModuleErrorKind::InvalidArgument,
            format!("{path} is not UTF-8 text"),
        )
    })
}

/// The lines of `text` from line `offset` (1-based; past the end: none),
/// at most `limit`: the first line's number, the lines, the total.
pub fn slice(text: &str, offset: Option<usize>, limit: Option<usize>) -> (usize, Vec<&str>, usize) {
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len();
    let start = (offset.unwrap_or(1).max(1) - 1).min(total);
    let end = limit.map_or(total, |limit| (start + limit).min(total));
    (start + 1, lines[start..end].to_vec(), total)
}

fn invalid(reason: &str) -> ModuleError {
    ModuleError::new(ModuleErrorKind::InvalidArgument, reason)
}

/// `content` with its one `old` replaced by `new`, and the line the change
/// starts on: an exact match first, else one whose lines match with their
/// trailing whitespace ignored.
///
/// # Errors
/// `InvalidArgument` when they are the same, or `old` matches nowhere or
/// more than once.
pub fn replace_once(content: &str, old: &str, new: &str) -> Result<(String, usize), ModuleError> {
    if old == new {
        return Err(invalid(
            "old_text and new_text are the same: nothing to change",
        ));
    }
    match content.matches(old).count() {
        1 => match content.split_once(old) {
            Some((before, after)) => Ok((
                format!("{before}{new}{after}"),
                before.matches('\n').count() + 1,
            )),
            None => Err(invalid("old_text is not in the file")),
        },
        0 => loose(content, old, new).ok_or_else(|| {
            invalid(
                "old_text is not in the file; match it exactly, whitespace and indentation \
                 included (read the file to check)",
            )
        }),
        n => Err(ModuleError::new(
            ModuleErrorKind::InvalidArgument,
            format!(
                "old_text is in the file {n} times; it must be there once (add context around it)"
            ),
        )),
    }
}

/// The lines of `text` without their trailing whitespace.
fn trimmed(text: &str) -> Vec<&str> {
    text.lines().map(str::trim_end).collect()
}

/// The replacement when `old` matches once with trailing whitespace of
/// each line ignored: the matched lines are replaced by `new`.
fn loose(content: &str, old: &str, new: &str) -> Option<(String, usize)> {
    let (lines, wanted) = (trimmed(content), trimmed(old));
    if wanted.is_empty() {
        return None;
    }
    let starts: Vec<usize> = (0..=lines.len().saturating_sub(wanted.len()))
        .filter(|&i| lines.get(i..i + wanted.len()) == Some(wanted.as_slice()))
        .collect();
    let [start] = starts.as_slice() else {
        return None;
    };
    let original: Vec<&str> = content.lines().collect();
    let mut out: Vec<String> = original[..*start]
        .iter()
        .map(|l| (*l).to_string())
        .collect();
    out.push(new.to_string());
    out.extend(
        original[start + wanted.len()..]
            .iter()
            .map(|l| (*l).to_string()),
    );
    let mut text = out.join("\n");
    if content.ends_with('\n') && !text.ends_with('\n') {
        text.push('\n');
    }
    Some((text, start + 1))
}

/// A line grep answers: its number, whether it matched (else it is
/// context), its text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrepLine {
    /// Its number (1-based).
    pub number: usize,
    /// Whether it matched.
    pub matched: bool,
    /// Its text.
    pub text: String,
}

/// The lines of `text` that `regex` matches, each with `context` lines
/// around it, and how many matched; at most `room` matches.
pub fn grep_lines(
    text: &str,
    regex: &Regex,
    context: usize,
    room: usize,
) -> (Vec<GrepLine>, usize) {
    let all: Vec<&str> = text.lines().collect();
    let mut out: Vec<GrepLine> = Vec::new();
    let mut hits = 0;
    for (n, line) in all.iter().enumerate() {
        if !regex.is_match(line) {
            continue;
        }
        if hits == room {
            // One more than there is room for: the caller says there are more.
            return (out, hits + 1);
        }
        hits += 1;
        let first = n.saturating_sub(context);
        let last = (n + context).min(all.len().saturating_sub(1));
        for (i, shown) in all.iter().enumerate().take(last + 1).skip(first) {
            match out.last_mut() {
                // Already given as context of the previous match.
                Some(prev) if prev.number == i + 1 => prev.matched |= i == n,
                Some(prev) if prev.number > i + 1 => {}
                _ => out.push(GrepLine {
                    number: i + 1,
                    matched: i == n,
                    text: (*shown).to_string(),
                }),
            }
        }
    }
    (out, hits)
}

#[cfg(test)]
mod tests;
