// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Names: a member's, the author it writes as, which a mention (`@name`)
//! names (a team's member, and in the cloud a key's agent, post-cortex-cloud
//! ARCHITECTURE C16); and a slug, what a module names its own things by (a
//! skill, a job, a source, a chat, a template, a workflow).

/// The most characters a name has.
pub const NAME_CHARS: usize = 64;

/// Whether `name` is a member's name: 1 to [`NAME_CHARS`] of A-Z, a-z, 0-9,
/// `.`, `_` and `-`.
#[must_use]
pub fn is_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= NAME_CHARS
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// Whether `name` is a slug: 1 to [`NAME_CHARS`] of a-z, 0-9 and `-`.
#[must_use]
pub fn is_slug(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= NAME_CHARS
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}
