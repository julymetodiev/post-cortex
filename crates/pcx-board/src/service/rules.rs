// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! How the service answers a rule the domain says is broken, and names the
//! tasks of a board a call names by a short id.

use pcx_module_api::{ModuleError, ModuleErrorKind};

use crate::domain::{self, Broken, Task};

/// The answer to a call that breaks `rule`.
pub fn broken(rule: Broken) -> ModuleError {
    let kind = match rule {
        Broken::Held { .. } | Broken::NotPending | Broken::NotApproved => {
            ModuleErrorKind::FailedPrecondition
        }
        Broken::NotTheDecider { .. } => ModuleErrorKind::Denied,
        Broken::Columns(_)
        | Broken::UnknownColumn { .. }
        | Broken::Empty(_)
        | Broken::SelfLink
        | Broken::NotOnBoard { .. }
        | Broken::Nested { .. }
        | Broken::IsPlan { .. }
        | Broken::Cycle { .. }
        | Broken::NotInColumn { .. }
        | Broken::LongKey { .. }
        | Broken::Ambiguous { .. } => ModuleErrorKind::InvalidArgument,
    };
    ModuleError::new(kind, rule.to_string())
}

/// `name` as the id of the task of `tasks` it names (a short id too).
pub fn on_board(name: &str, tasks: &[(&str, &Task)]) -> Result<String, ModuleError> {
    domain::full(name, tasks.iter().map(|(id, _)| *id)).map_err(broken)
}

/// Each of `names` as the id of the task of `tasks` it names.
pub fn all_on_board(names: &[String], tasks: &[(&str, &Task)]) -> Result<Vec<String>, ModuleError> {
    names.iter().map(|n| on_board(n, tasks)).collect()
}
