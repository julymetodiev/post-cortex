// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A provider's listing (docs/modules/agent.md, the models): the models its
//! API serves, each as its table knows it (else a chat model on text with
//! its window unknown); every one may be named.

use crate::model::{Modality, ModelFailure, ModelFailureKind, ModelInfo, ModelKind};

/// A model a provider serves that its table does not know: a chat model
/// on text, its window and output unknown (0), named by its id.
#[must_use]
pub fn unknown(id: &str) -> ModelInfo {
    ModelInfo {
        id: id.to_string(),
        kind: ModelKind::Chat.into(),
        input: vec![Modality::Text.into()],
        output: vec![Modality::Text.into()],
        ..ModelInfo::default()
    }
}

/// What a provider knows of its models without its API.
#[derive(Debug, Clone, Default)]
pub struct Table {
    /// The models it knows.
    pub known: Vec<ModelInfo>,
}

impl Table {
    /// The listing: `served` (the models the API gives, in its order;
    /// none: the table's), each as the table knows it where it does.
    #[must_use]
    pub fn listing(&self, served: Option<Vec<ModelInfo>>) -> Vec<ModelInfo> {
        match served {
            None => self.known.clone(),
            Some(served) => served
                .into_iter()
                .map(|model| {
                    self.known
                        .iter()
                        .find(|k| k.id == model.id)
                        .cloned()
                        .unwrap_or(model)
                })
                .collect(),
        }
    }

    /// The listing of a provider at its start, from what its API answered
    /// (`served`): a refused key fails the start; any other failure (the
    /// API down) leaves the table's models.
    ///
    /// # Errors
    /// `served` failed `Unauthorized`.
    pub fn listed(
        &self,
        provider: &str,
        served: Result<Vec<ModelInfo>, ModelFailure>,
    ) -> Result<Vec<ModelInfo>, ModelFailure> {
        let served = match served {
            Ok(served) => Some(served),
            Err(failure) if failure.kind == ModelFailureKind::Unauthorized => return Err(failure),
            Err(failure) => {
                tracing::warn!(
                    provider,
                    error = %failure.message,
                    "its models could not be listed; its table's are"
                );
                None
            }
        };
        Ok(self.listing(served))
    }
}

#[cfg(test)]
mod tests;
