// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Slugs: the segment of an address that names a workspace (and, in the
//! hosted cloud, an organisation or a connected daemon). A slug is made
//! once, from the name, when its owner is created, and kept after a rename,
//! so a link always opens what it named. Before, the app made them from the
//! names in list order, so deleting one of two same-named workspaces moved
//! the other's address.

use uuid::Uuid;

/// The longest base a name gives.
const MAX_BASE: usize = 48;

/// The slug `name` asks for: its ASCII letters (lowercased) and digits,
/// every other run as one `-`, none at either end, at most 48 characters;
/// `empty` when nothing is left.
pub fn slug_base(name: &str, empty: &str) -> String {
    let mut base = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            base.push(c.to_ascii_lowercase());
        } else if !base.is_empty() && !base.ends_with('-') {
            base.push('-');
        }
    }
    base.truncate(MAX_BASE);
    let base = base.trim_end_matches('-');
    if base.is_empty() {
        empty.to_string()
    } else {
        base.to_string()
    }
}

/// `base` made unique by `id`: the base and the first six hex digits of the
/// id, for a base another owner has.
pub fn slug_with_id(base: &str, id: Uuid) -> String {
    let hex = id.simple().to_string();
    format!("{base}-{}", &hex[..6])
}

/// The slug of the new `id` named `name`: the base of the name (`empty`
/// when nothing is left), or the base with the id when another owner has
/// the base (`taken`).
pub fn unique_slug(name: &str, empty: &str, id: Uuid, taken: impl FnOnce(&str) -> bool) -> String {
    let base = slug_base(name, empty);
    if taken(&base) {
        slug_with_id(&base, id)
    } else {
        base
    }
}

/// The slug of the new workspace `id` named `name` ([`unique_slug`];
/// `workspace` for a name with no letter or digit).
pub fn workspace_slug(name: &str, id: Uuid, taken: impl FnOnce(&str) -> bool) -> String {
    unique_slug(name, "workspace", id, taken)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_gives_lowercase_letters_and_digits_joined_by_dashes() {
        assert_eq!(slug_base("Acme Robotics", "x"), "acme-robotics");
        assert_eq!(slug_base("  R&D / 2026 ", "x"), "r-d-2026");
        assert_eq!(slug_base("Ünï", "x"), "n");
        assert_eq!(slug_base("Моята фирма", "personal"), "personal");
        assert_eq!(slug_base(&"a".repeat(60), "x").len(), 48);
        assert_eq!(
            slug_base(&format!("{}-b", "a".repeat(47)), "x"),
            "a".repeat(47)
        );
    }

    #[test]
    fn a_taken_base_carries_the_id() {
        let id = Uuid::parse_str("1f2e3d4c-0000-4000-8000-000000000000").unwrap();
        assert_eq!(workspace_slug("Checkout", id, |_| false), "checkout");
        assert_eq!(
            workspace_slug("Checkout", id, |b| b == "checkout"),
            "checkout-1f2e3d"
        );
        assert_eq!(workspace_slug("", id, |_| false), "workspace");
    }
}
