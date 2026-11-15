// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The skills service: the library's operations over its documents, the
//! domain's rules applied to what they read and write (ARCHITECTURE
//! section 8). Between the handlers (tools and prompts) and the port.

use pcx_documents::{Documents, Item, Typed};
use pcx_module_api::{ModuleError, ModuleErrorKind};

use crate::domain::{self, Broken, Revision, Skill};

/// A skill's prompt, filled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillPrompt {
    /// What it is for.
    pub description: String,
    /// Its one message.
    pub text: String,
}

fn broken(rule: Broken) -> ModuleError {
    ModuleError::new(ModuleErrorKind::InvalidArgument, rule.to_string())
}

fn not_found(name: &str) -> ModuleError {
    ModuleError::new(
        ModuleErrorKind::NotFound,
        format!("no skill {name} in the library"),
    )
}

/// The library's operations, over its documents in its workspace.
pub struct Service<'a> {
    docs: &'a dyn Documents,
    /// `skills/skill`.
    kind: String,
}

impl<'a> Service<'a> {
    /// The library over `docs`.
    #[must_use]
    pub fn new(docs: &'a dyn Documents) -> Self {
        Self {
            docs,
            kind: domain::document(),
        }
    }

    /// Add a skill.
    ///
    /// # Errors
    /// `InvalidArgument` when it breaks a rule; `Conflict` when its name is
    /// taken; memory failed.
    pub async fn add(
        &self,
        name: &str,
        description: &str,
        instructions: &str,
        tools: Vec<String>,
    ) -> Result<Item<Skill>, ModuleError> {
        let (skill, instructions) =
            domain::skill(name, description, instructions, tools).map_err(broken)?;
        if self.find(&skill.name).await?.is_some() {
            return Err(ModuleError::new(
                ModuleErrorKind::Conflict,
                format!("the library has a skill {} already: update it", skill.name),
            ));
        }
        self.skills().create(&skill, &instructions).await
    }

    /// Change skill `name` from its `version`.
    ///
    /// # Errors
    /// `NotFound` for no such skill; `InvalidArgument` when the change
    /// breaks a rule; `Conflict` when it changed since `version`; memory
    /// failed.
    pub async fn update(
        &self,
        name: &str,
        version: u64,
        revision: Revision,
    ) -> Result<Item<Skill>, ModuleError> {
        let current = self.get(name).await?;
        let (skill, instructions) =
            domain::revise(&current.data, &current.document.text, revision).map_err(broken)?;
        self.skills()
            .change(&current.document.id, version, &skill, &instructions)
            .await
    }

    /// The built-in skills the library has not, added, and those it has
    /// as the module last wrote them, renewed to the text shipped now; a
    /// skill whose last version someone else wrote (a person, an agent) is
    /// left as it is (docs/modules/skills.md, built-in skills): the names
    /// added or renewed.
    ///
    /// # Errors
    /// `Internal` for a built-in skill that does not read; memory failed.
    pub async fn seed(&self) -> Result<Vec<String>, ModuleError> {
        let mut written = Vec::new();
        for file in domain::LIBRARY {
            let (skill, instructions) = domain::shipped(file)
                .map_err(|rule| ModuleError::new(ModuleErrorKind::Internal, rule.to_string()))?;
            match self.find(&skill.name).await? {
                None => {
                    self.skills().create(&skill, &instructions).await?;
                }
                Some(kept)
                    if kept.document.author.as_deref() == Some(domain::MODULE_ID)
                        && (kept.data != skill || kept.document.text != instructions) =>
                {
                    self.skills()
                        .change(
                            &kept.document.id,
                            kept.document.version,
                            &skill,
                            &instructions,
                        )
                        .await?;
                }
                Some(_) => continue,
            }
            written.push(skill.name);
        }
        Ok(written)
    }

    /// Every skill, by name.
    ///
    /// # Errors
    /// Memory failed.
    pub async fn list(&self) -> Result<Vec<Item<Skill>>, ModuleError> {
        let mut all: Vec<Item<Skill>> = self.skills().all(None).await?;
        all.sort_by(|a, b| a.data.name.cmp(&b.data.name));
        Ok(all)
    }

    /// Skill `name`.
    ///
    /// # Errors
    /// `NotFound` for no such skill; memory failed.
    pub async fn get(&self, name: &str) -> Result<Item<Skill>, ModuleError> {
        self.find(name).await?.ok_or_else(|| not_found(name))
    }

    /// Skill `name`'s prompt.
    ///
    /// # Errors
    /// As [`Service::get`].
    pub async fn prompt(&self, name: &str) -> Result<SkillPrompt, ModuleError> {
        let skill = self.get(name).await?;
        Ok(SkillPrompt {
            text: domain::prompt(&skill.data, &skill.document.text),
            description: skill.data.description,
        })
    }

    async fn find(&self, name: &str) -> Result<Option<Item<Skill>>, ModuleError> {
        let named = Skill {
            name: name.to_string(),
            ..Skill::default()
        };
        let found: Vec<Item<Skill>> = self.skills().all(Some((&named, &["name"]))).await?;
        Ok(found.into_iter().next())
    }

    fn skills(&self) -> Typed<'_> {
        Typed::new(self.docs, self.kind.as_str())
    }
}

#[cfg(test)]
mod tests;
