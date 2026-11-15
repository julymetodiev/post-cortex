// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Text a tool answers, kept to what an agent can read: the first or the
//! last lines of a long text, at most so many lines and bytes.

/// Whole lines of a text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Excerpt {
    /// The lines kept.
    pub text: String,
    /// Lines kept.
    pub shown: usize,
    /// Lines of the whole text.
    pub total: usize,
}

impl Excerpt {
    /// Whether lines were left out.
    #[must_use]
    pub fn truncated(&self) -> bool {
        self.shown < self.total
    }
}

/// The first lines of `text`, at most `max_lines` lines and `max_bytes`
/// bytes (whole lines only).
#[must_use]
pub fn head(text: &str, max_lines: usize, max_bytes: usize) -> Excerpt {
    let kept = keep(text.lines(), max_lines, max_bytes);
    excerpt(text, &kept)
}

/// The last lines of `text`, at most `max_lines` lines and `max_bytes`
/// bytes (whole lines only): what a command printed last.
#[must_use]
pub fn tail(text: &str, max_lines: usize, max_bytes: usize) -> Excerpt {
    let mut kept = keep(text.lines().rev(), max_lines, max_bytes);
    kept.reverse();
    excerpt(text, &kept)
}

/// The lines of `lines` in their order until `max_lines` lines or
/// `max_bytes` bytes (joined by newlines) would be passed.
fn keep<'a>(
    lines: impl Iterator<Item = &'a str>,
    max_lines: usize,
    max_bytes: usize,
) -> Vec<&'a str> {
    let mut kept = Vec::new();
    let mut bytes = 0;
    for line in lines {
        let len = if kept.is_empty() {
            line.len()
        } else {
            bytes + 1 + line.len()
        };
        if kept.len() >= max_lines || len > max_bytes {
            break;
        }
        kept.push(line);
        bytes = len;
    }
    kept
}

fn excerpt(text: &str, kept: &[&str]) -> Excerpt {
    Excerpt {
        text: kept.join("\n"),
        shown: kept.len(),
        total: text.lines().count(),
    }
}

/// `text` cut to `max` characters, with an ellipsis when cut.
#[must_use]
pub fn clip(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((at, _)) => format!("{}...", text[..at].trim_end()),
        None => text.to_string(),
    }
}

#[cfg(test)]
mod tests;
