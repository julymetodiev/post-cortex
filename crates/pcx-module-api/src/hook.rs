// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The event kinds of Claude Code and Codex hook names.

use crate::EventKind;

/// The kind of the event a hook names (`hook_event_name`); none for a hook
/// no module answers.
#[must_use]
pub fn event_kind(hook_event_name: &str) -> Option<EventKind> {
    Some(match hook_event_name {
        "SessionStart" => EventKind::SessionStart,
        "SessionEnd" => EventKind::SessionEnd,
        "UserPromptSubmit" => EventKind::TurnStart,
        "Stop" => EventKind::TurnEnd,
        "PreToolUse" => EventKind::PreTool,
        "PostToolUse" => EventKind::PostTool,
        "PreCompact" => EventKind::PreCompact,
        _ => return None,
    })
}
