// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The chief's settings as its configuration gives them (ARCHITECTURE
//! section 8, its settings); its grant (`person_tools`, `withheld`) the
//! host enforces, the chief only shows it.

use pcx_module_api::agent::TurnGrant;
use pcx_module_api::{ModuleError, ModuleErrorKind};
use post_cortex_proto::chief::ChiefConfig;

/// Its author when its settings name none.
pub const DEFAULT_NAME: &str = "chief";

/// Its tokens per day when its settings set none.
pub const DEFAULT_BUDGET: u64 = 2_000_000;

/// What the chief runs with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// The workspace its conversations, actions and days live in.
    pub workspace: String,
    /// Its name, the author of what it writes.
    pub name: String,
    /// Added to the agent's system prompt; empty: none.
    pub instructions: String,
    /// Its model; none: the agent's.
    pub model: Option<String>,
    /// Its tokens per day.
    pub budget: u64,
    /// Its grant, as the host reads it.
    pub grant: TurnGrant,
}

impl Settings {
    /// The settings of `config`, which the host has checked against its
    /// message.
    ///
    /// # Errors
    /// `InvalidArgument` for a missing workspace.
    pub fn of(config: ChiefConfig) -> Result<Self, ModuleError> {
        if config.workspace_id.is_empty() {
            return Err(ModuleError::new(
                ModuleErrorKind::InvalidArgument,
                "the chief's setting workspace_id is the workspace its conversations live in",
            )
            .with_field("workspace_id"));
        }
        Ok(Self {
            workspace: config.workspace_id,
            name: Some(config.name)
                .filter(|n| !n.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_NAME.to_string()),
            instructions: config.instructions,
            model: Some(config.model).filter(|m| !m.trim().is_empty()),
            budget: if config.budget_tokens == 0 {
                DEFAULT_BUDGET
            } else {
                config.budget_tokens
            },
            grant: config.grant.map(TurnGrant::from).unwrap_or_default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_settings_default_all_but_the_workspace() {
        let settings = Settings::of(ChiefConfig {
            workspace_id: "w1".to_string(),
            ..ChiefConfig::default()
        })
        .unwrap();
        assert_eq!(
            settings,
            Settings {
                workspace: "w1".to_string(),
                name: DEFAULT_NAME.to_string(),
                instructions: String::new(),
                model: None,
                budget: DEFAULT_BUDGET,
                grant: TurnGrant::default(),
            }
        );
        let refused = Settings::of(ChiefConfig::default()).unwrap_err();
        assert_eq!(refused.kind, ModuleErrorKind::InvalidArgument);
        assert_eq!(refused.field.as_deref(), Some("workspace_id"));
    }
}
