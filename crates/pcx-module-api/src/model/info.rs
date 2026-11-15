// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A model's description (`pcx.module.v1.ModelInfo`, ARCHITECTURE section
//! 7, the agent): the one check of its shape, which the host runs on every
//! provider's models at load and a client runs on the models it reads.

use post_cortex_proto::module::{Modality, ModelFeature, ModelInfo, ModelKind};

use crate::{ModuleError, ModuleErrorKind};

/// A model whose description is out of shape; the provider that lists it is
/// refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ModelInfoError {
    /// It has no id.
    #[error("a model has no id")]
    NoId,
    /// Its kind is unset or not one the host knows.
    #[error("model {model}: its kind is not set")]
    Kind {
        /// The model.
        model: String,
    },
    /// It takes or gives nothing.
    #[error("model {model}: it names no {side} modality")]
    NoModality {
        /// The model.
        model: String,
        /// `input` or `output`.
        side: &'static str,
    },
    /// A modality or feature is unset or not one the host knows.
    #[error("model {model}: {field} holds a value that is not set ({value})")]
    Unknown {
        /// The model.
        model: String,
        /// `input`, `output` or `features`.
        field: &'static str,
        /// The value on the wire.
        value: i32,
    },
}

/// Check one model's description: an id, a kind, at least one modality in
/// and out, and no unset modality or feature.
///
/// # Errors
/// What is out of shape, naming the model.
pub fn check(info: &ModelInfo) -> Result<(), ModelInfoError> {
    if info.id.is_empty() {
        return Err(ModelInfoError::NoId);
    }
    let model = || info.id.clone();
    if !matches!(ModelKind::try_from(info.kind), Ok(k) if k != ModelKind::Unspecified) {
        return Err(ModelInfoError::Kind { model: model() });
    }
    for (side, values) in [("input", &info.input), ("output", &info.output)] {
        if values.is_empty() {
            return Err(ModelInfoError::NoModality {
                model: model(),
                side,
            });
        }
        if let Some(&value) = values
            .iter()
            .find(|&&v| !matches!(Modality::try_from(v), Ok(m) if m != Modality::Unspecified))
        {
            return Err(ModelInfoError::Unknown {
                model: model(),
                field: side,
                value,
            });
        }
    }
    if let Some(&value) = info
        .features
        .iter()
        .find(|&&v| !matches!(ModelFeature::try_from(v), Ok(f) if f != ModelFeature::Unspecified))
    {
        return Err(ModelInfoError::Unknown {
            model: model(),
            field: "features",
            value,
        });
    }
    Ok(())
}

impl From<ModelInfoError> for ModuleError {
    /// `Internal`: the side that listed the model answered it out of shape.
    fn from(e: ModelInfoError) -> Self {
        ModuleError::new(ModuleErrorKind::Internal, e.to_string())
    }
}

/// Check every model of a listing read from the wire.
///
/// # Errors
/// The first model out of shape.
pub fn checked(models: Vec<ModelInfo>) -> Result<Vec<ModelInfo>, ModelInfoError> {
    models.iter().try_for_each(check)?;
    Ok(models)
}

#[cfg(test)]
mod tests;
