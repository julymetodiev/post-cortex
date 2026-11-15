// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The context engine service: recall through memory; a summary through
//! the model, kept in memory.

use pcx_module_api::model::{ModelMessage, ModelProvider};
use pcx_module_api::{ModuleError, ModuleErrorKind, Scope};

use crate::domain;
use crate::ports::Memory;

/// The context engine over memory.
pub struct Engine<'a> {
    /// Memory.
    pub memory: &'a dyn Memory,
}

impl Engine<'_> {
    /// What memory recalls for `prompt` in `session`; empty when nothing.
    ///
    /// # Errors
    /// Memory failed.
    pub async fn recall(&self, session: &str, prompt: &str) -> Result<String, ModuleError> {
        let (items, text) = self.memory.recall(session, prompt).await?;
        Ok(domain::recalled(items, text))
    }

    /// `messages` summed up by `model` of `models` in `scope`, kept as a
    /// record of `session`.
    ///
    /// # Errors
    /// `Unavailable` when the model failed or said nothing; memory failed.
    pub async fn summarize(
        &self,
        models: &dyn ModelProvider,
        scope: &Scope,
        session: &str,
        model: &str,
        messages: &[ModelMessage],
    ) -> Result<String, ModuleError> {
        let unreached = |message: String| ModuleError::new(ModuleErrorKind::Unavailable, message);
        let request = domain::summary_request(model, messages);
        let (blocks, _, _) = models
            .generate(scope, request)
            .await
            .map_err(|e| unreached(format!("the summary's model call: {e}")))?
            .collect()
            .await
            .map_err(|e| unreached(format!("the summary's model call: {e}")))?;
        let summary = domain::summary_of(&blocks)
            .ok_or_else(|| unreached("the model gave no summary".to_string()))?;
        self.memory.keep_summary(session, &summary).await?;
        Ok(summary)
    }
}

#[cfg(test)]
mod tests;
