// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What a vector embeds.

use serde::{Deserialize, Serialize};

use crate::core::wire::wire_names;

/// Kind of vectorised content. Its name is stored with every vector and
/// returned by search, so the wire name is the stored (variant) name.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum ContentKind {
    /// Update content from context updates
    UpdateContent,
    /// Entity descriptions
    EntityDescription,
    /// User questions and messages
    UserMessage,
    /// Decision points and rationale
    DecisionPoint,
    /// Code snippets and technical content
    CodeSnippet,
    /// Problem-solution pairs
    ProblemSolution,
    /// General session metadata
    SessionMetadata,
    /// A raw conversation turn from the message stream
    /// ([`crate::core::message`])
    Message,
    /// A module's document ([`crate::core::document`]), its latest version
    Document,
}

wire_names!(ContentKind, "content kind", {
    UpdateContent => "UpdateContent",
    EntityDescription => "EntityDescription",
    UserMessage => "UserMessage",
    DecisionPoint => "DecisionPoint",
    CodeSnippet => "CodeSnippet",
    ProblemSolution => "ProblemSolution",
    SessionMetadata => "SessionMetadata",
    Message => "Message",
    Document => "Document",
});

impl ContentKind {
    /// Whether the content embeds a context update (a record), not an
    /// entity description or a captured message.
    pub const fn is_record(&self) -> bool {
        !matches!(
            self,
            Self::EntityDescription | Self::Message | Self::Document
        )
    }

    /// Importance weight of this kind in search scoring.
    #[must_use]
    pub const fn importance_weight(&self) -> f32 {
        match self {
            Self::DecisionPoint => 1.0,
            Self::ProblemSolution => 0.9,
            Self::UserMessage | Self::Document => 0.8,
            Self::UpdateContent => 0.7,
            Self::CodeSnippet => 0.6,
            Self::EntityDescription => 0.5,
            Self::Message => 0.4,
            Self::SessionMetadata => 0.3,
        }
    }
}
