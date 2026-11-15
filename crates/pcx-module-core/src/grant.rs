// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What a module's settings give the turns it asks of the agent
//! (ARCHITECTURE section 7, a module's turns): the tools only a person
//! calls that it may call by name, and the tools it is not given: the
//! `pcx.module.v1.TurnGrant` of its configuration message
//! ([`crate::ConfigType::grant`]). The host reads it when it loads the
//! module and checks every call of its turns.

use post_cortex_proto::module as wire;

/// The grant of a module's turns.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TurnGrant {
    /// Tools only a person calls (visibility `app`) its turns may call.
    pub person_tools: Vec<String>,
    /// Tools its turns are not given, whatever else allows them.
    pub withheld: Vec<String>,
}

impl From<wire::TurnGrant> for TurnGrant {
    fn from(grant: wire::TurnGrant) -> Self {
        Self {
            person_tools: grant.person_tools,
            withheld: grant.withheld,
        }
    }
}

impl TurnGrant {
    /// Whether its turns may call tool `tool`, one an agent may call
    /// (`for_agents`) or one only a person calls.
    #[must_use]
    pub fn allows(&self, tool: &str, for_agents: bool) -> bool {
        let named = |list: &[String]| list.iter().any(|t| t == tool);
        !named(&self.withheld) && (for_agents || named(&self.person_tools))
    }
}

#[cfg(test)]
mod tests;
