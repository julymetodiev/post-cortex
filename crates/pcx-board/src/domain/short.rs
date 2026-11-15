// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A task named by a short id: the start of its id, as the app shows it.

use crate::domain::Broken;

/// The fewest characters of a task's id a short id has.
pub const SHORT_ID: usize = 8;

/// The id among `ids` that `name` names: itself when it is one of them,
/// else the one it begins when it has at least [`SHORT_ID`] characters;
/// none when it names none of them.
///
/// # Errors
/// It begins more than one of them.
pub fn task_id<'a>(
    name: &str,
    ids: impl IntoIterator<Item = &'a str> + Clone,
) -> Result<Option<String>, Broken> {
    if ids.clone().into_iter().any(|id| id == name) {
        return Ok(Some(name.to_string()));
    }
    if name.chars().count() < SHORT_ID {
        return Ok(None);
    }
    let mut begun: Vec<&str> = ids.into_iter().filter(|id| id.starts_with(name)).collect();
    match begun.len() {
        0 => Ok(None),
        1 => Ok(begun.pop().map(String::from)),
        _ => {
            begun.sort_unstable();
            Err(Broken::Ambiguous {
                short: name.to_string(),
                tasks: begun.into_iter().map(String::from).collect(),
            })
        }
    }
}

/// `name` as the full id among `ids` it names; unchanged when it names
/// none, so the rule that needs the task says it is not there.
///
/// # Errors
/// It begins more than one of them.
pub fn full<'a>(
    name: &str,
    ids: impl IntoIterator<Item = &'a str> + Clone,
) -> Result<String, Broken> {
    Ok(task_id(name, ids)?.unwrap_or_else(|| name.to_string()))
}
