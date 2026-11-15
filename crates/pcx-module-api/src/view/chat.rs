// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The check of a chat (W25).

use serde_json::Value;

use crate::view::{Chat, Checker, Slot, ViewError};

impl Checker<'_> {
    /// A chat: with the host's agent, or with the module's own turns (a
    /// module that runs them), its member by name or by the picked row.
    pub fn chat(&self, chat: &Chat, at: &str) -> Result<(), ViewError> {
        let own = chat.agent == self.module;
        if !chat.agent.is_empty() && !own {
            return Err(self.refused(
                at,
                format!(
                    "chats with {}: a view chats with the host's agent or its own module",
                    chat.agent
                ),
            ));
        }
        if own && !self.turns {
            return Err(self.refused(at, "chats with a module that runs no turns"));
        }
        if chat.member.is_empty() {
            return Ok(());
        }
        if !own {
            return Err(self.refused(at, "names a member of no module"));
        }
        self.template(
            &Value::String(chat.member.clone()),
            at,
            Slot {
                row: self.detail,
                ..Slot::NONE
            },
        )
    }
}

#[cfg(test)]
mod tests;
