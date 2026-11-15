// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The document kinds port: the kinds the running modules declare
//! (ARCHITECTURE section 7, modules together). Memory reads them on every
//! write of a document; its adapter reads the host's manifests.

use async_trait::async_trait;
use post_cortex_core::core::document::KindDeclaration;

/// Why the declared kinds could not be read.
#[derive(Debug, thiserror::Error)]
pub enum KindsError {
    /// The kinds cannot be read yet (memory has no host before its start).
    #[error("{0}")]
    NotReady(String),
    /// The host did not answer.
    #[error("{0}")]
    Unavailable(String),
    /// A running module declares a kind that does not read.
    #[error("{0}")]
    Invalid(String),
}

/// The document kinds of the running modules.
#[async_trait]
pub trait DocumentKinds: Send + Sync {
    /// The kinds declared now.
    ///
    /// # Errors
    /// See [`KindsError`].
    async fn kinds(&self) -> Result<Vec<KindDeclaration>, KindsError>;
}
