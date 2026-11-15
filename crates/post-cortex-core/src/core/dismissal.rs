// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Radar dismissals: items of the `radar` graph view a workspace has
//! looked at and set aside.
//!
//! The radar lists what needs attention (unsolved problems, decisions
//! without a rationale, open questions, stale entities, possible aliases).
//! Some items are fine as they are; dismissing one hides it from the
//! workspace's radar (and the radar of its member sessions) until it is
//! restored. A dismissal names the item by its kind and a key:
//!
//! | kind | key |
//! |------|-----|
//! | `unsolved_problem` | entity name |
//! | `decision_without_rationale` | record (entry) id |
//! | `open_question` | question text |
//! | `stale_entity` | entity name |
//! | `alias_candidate` | `<name>\|<of>` (the two entity names) |

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Longest dismissal key, in bytes.
pub const MAX_DISMISSAL_KEY_BYTES: usize = 1024;

/// The list of the radar an item is on.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum RadarItemKind {
    /// A problem entity nothing `solves`.
    UnsolvedProblem,
    /// A valid decision recorded without a rationale.
    DecisionWithoutRationale,
    /// A question still open.
    OpenQuestion,
    /// An entity untouched for months.
    StaleEntity,
    /// Two entities that may be one.
    AliasCandidate,
}

crate::core::wire::wire_names!(RadarItemKind, "radar kind", {
    UnsolvedProblem => "unsolved_problem",
    DecisionWithoutRationale => "decision_without_rationale",
    OpenQuestion => "open_question",
    StaleEntity => "stale_entity",
    AliasCandidate => "alias_candidate",
});

/// The key of an alias candidate: `<name>|<of>`.
pub fn alias_key(name: &str, of: &str) -> String {
    format!("{name}|{of}")
}

/// A radar item a workspace dismissed.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct RadarDismissal {
    /// The radar list the item is on.
    pub kind: RadarItemKind,
    /// Which item (see the module docs).
    pub key: String,
    /// When it was dismissed.
    pub dismissed_at: DateTime<Utc>,
    /// Who dismissed it, when known.
    pub author: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_round_trip() {
        for &kind in RadarItemKind::ALL {
            assert_eq!(kind.as_str().parse::<RadarItemKind>(), Ok(kind));
            assert_eq!(
                serde_json::to_value(kind).unwrap(),
                serde_json::Value::String(kind.as_str().to_string())
            );
        }
        // Strict: tolerant matching of client input is the transport's.
        assert!(" Open_Question ".parse::<RadarItemKind>().is_err());
        assert!("nope".parse::<RadarItemKind>().is_err());
    }
}
