// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The skills' domain (ARCHITECTURE section 8): a skill as its document
//! holds it (its data; its instructions are the document's text), the
//! rules of its name and fields, and its prompt. Pure: no host, no memory.

use std::fmt;

/// The module's id, and the prefix of its document kind.
pub const MODULE_ID: &str = "skills";

/// Its one document kind in the module.
pub const KIND: &str = "skill";

/// Longest name a skill may have.
pub use pcx_module_api::name::NAME_CHARS;

/// The full name of the kind, `skills/skill`, as memory keeps it.
#[must_use]
pub fn document() -> String {
    format!("{MODULE_ID}/{KIND}")
}

/// A skill's data, the message of `pcx.skills.v1`; its instructions
/// (Markdown) are its document's text. The tools it needs are not checked:
/// a skill may name a module that does not run yet.
pub use post_cortex_proto::skills::Skill;

/// A rule a skill breaks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Broken {
    /// Its name is empty, too long, or has a character outside `[a-z0-9-]`.
    Name(String),
    /// A field that must have text has none.
    Empty(&'static str),
    /// A built-in skill's file is not a front matter and instructions.
    Shipped(String),
}

impl fmt::Display for Broken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Name(name) => write!(
                f,
                "a skill's name is 1 to {NAME_CHARS} of a-z, 0-9 and -, not \"{name}\""
            ),
            Self::Empty(field) => write!(f, "a skill's {field} is empty"),
            Self::Shipped(why) => write!(f, "a built-in skill does not read: {why}"),
        }
    }
}

/// The skills the crate ships (docs/modules/skills.md, built-in skills),
/// each a front matter (`name`, `description`, `tools`) and instructions.
pub const LIBRARY: [&str; 2] = [
    include_str!("../library/workflow-builder.md"),
    include_str!("../library/module-builder.md"),
];

/// A built-in skill's `file` read: its skill and instructions.
///
/// # Errors
/// [`Broken::Shipped`] for a file without a front matter or with a line of
/// it that is not `key: value` of a known key; the skill's rules.
pub fn shipped(file: &str) -> Result<(Skill, String), Broken> {
    let rest = file
        .strip_prefix("---\n")
        .ok_or_else(|| Broken::Shipped("it does not start with ---".into()))?;
    let (front, instructions) = rest
        .split_once("\n---\n")
        .ok_or_else(|| Broken::Shipped("its front matter does not end with ---".into()))?;
    let (mut named, mut description, mut needs) = (None, None, Vec::new());
    for line in front.lines().filter(|l| !l.trim().is_empty()) {
        let (key, value) = line
            .split_once(':')
            .ok_or_else(|| Broken::Shipped(format!("\"{line}\" is not key: value")))?;
        let value = value.trim();
        match key.trim() {
            "name" => named = Some(value),
            "description" => description = Some(value),
            "tools" => needs = value.split(',').map(|t| t.trim().to_string()).collect(),
            other => return Err(Broken::Shipped(format!("{other} is no key of a skill"))),
        }
    }
    skill(
        named.ok_or_else(|| Broken::Shipped("it names no name".into()))?,
        description.unwrap_or_default(),
        instructions,
        needs,
    )
}

/// `name` if it is a skill's name.
///
/// # Errors
/// [`Broken::Name`].
pub fn name(name: &str) -> Result<&str, Broken> {
    if pcx_module_api::name::is_slug(name) {
        Ok(name)
    } else {
        Err(Broken::Name(name.to_string()))
    }
}

/// `text` trimmed, if it has any.
fn filled(field: &'static str, text: &str) -> Result<String, Broken> {
    let text = text.trim();
    if text.is_empty() {
        Err(Broken::Empty(field))
    } else {
        Ok(text.to_string())
    }
}

/// `tools` trimmed, each once, in order.
fn tools(tools: Vec<String>) -> Result<Vec<String>, Broken> {
    let mut kept: Vec<String> = Vec::new();
    for tool in tools {
        let tool = filled("tool name", &tool)?;
        if !kept.contains(&tool) {
            kept.push(tool);
        }
    }
    Ok(kept)
}

/// A new skill and its instructions.
///
/// # Errors
/// The name, description or instructions break a rule.
pub fn skill(
    named: &str,
    description: &str,
    instructions: &str,
    needs: Vec<String>,
) -> Result<(Skill, String), Broken> {
    let skill = Skill {
        name: name(named)?.to_string(),
        description: filled("description", description)?,
        tools: tools(needs)?,
    };
    Ok((skill, filled("instructions", instructions)?))
}

/// What a change of a skill gives; none of a field: unchanged. Its name
/// does not change: it is its prompt's.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Revision {
    /// Its description.
    pub description: Option<String>,
    /// Its instructions.
    pub instructions: Option<String>,
    /// The tools it needs.
    pub tools: Option<Vec<String>>,
}

/// `current` (with its `instructions`) changed by `revision`.
///
/// # Errors
/// A changed field breaks a rule.
pub fn revise(
    current: &Skill,
    instructions: &str,
    revision: Revision,
) -> Result<(Skill, String), Broken> {
    let skill = Skill {
        name: current.name.clone(),
        description: match revision.description {
            Some(d) => filled("description", &d)?,
            None => current.description.clone(),
        },
        tools: match revision.tools {
            Some(t) => tools(t)?,
            None => current.tools.clone(),
        },
    };
    let instructions = match revision.instructions {
        Some(i) => filled("instructions", &i)?,
        None => instructions.to_string(),
    };
    Ok((skill, instructions))
}

/// The one message of `skill`'s prompt: its instructions, then the tools
/// it needs.
#[must_use]
pub fn prompt(skill: &Skill, instructions: &str) -> String {
    if skill.tools.is_empty() {
        instructions.to_string()
    } else {
        format!(
            "{instructions}\n\nTools this skill needs: {}.",
            skill.tools.join(", ")
        )
    }
}

#[cfg(test)]
mod tests;
