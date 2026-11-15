// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The context engine (ARCHITECTURE section 7, the agent): the module in
//! the `context_engine` slot gives a turn what memory recalls for it and
//! sums up a conversation too long for the model ([`ContextEngine`], from
//! [`crate::Module::context_engine`]); the agent reaches it through its
//! host handle ([`crate::ModuleHost::context`]).

use async_trait::async_trait;

use crate::model::ModelMessage;
use crate::{ModuleError, Scope};

mod remote;
mod serve;

pub use remote::RemoteContext;
pub use serve::{ContextService, ContextSource};

/// The context of the agent's turns.
#[async_trait]
pub trait ContextEngine: Send + Sync {
    /// What memory recalls for `prompt` in `session`, as text the turn's
    /// system prompt carries; empty when it recalls nothing.
    ///
    /// # Errors
    /// Memory failed.
    async fn recall(
        &self,
        scope: &Scope,
        session: &str,
        prompt: &str,
    ) -> Result<String, ModuleError>;

    /// `messages` of `session` summed up by `model`, kept as a record of
    /// the session too: the summary the conversation goes on from.
    ///
    /// # Errors
    /// The model or memory failed.
    async fn summarize(
        &self,
        scope: &Scope,
        session: &str,
        model: &str,
        messages: &[ModelMessage],
    ) -> Result<String, ModuleError>;
}
