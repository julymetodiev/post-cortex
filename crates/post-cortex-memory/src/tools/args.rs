// Copyright (c) 2025, 2026 Julius ML
// MIT License

//! The argument types of the nine tools; their schemars attributes are the
//! published input schemas (`tools/list`).

mod admin;
mod assembly;
mod document;
mod entity;
mod messages;
mod query;
mod search;
mod session;
mod summary;
mod update_context;
mod workspace;

pub use admin::AdminRequest;
pub use assembly::AssembleContextRequest;
pub use document::ManageDocumentRequest;
pub use entity::ManageEntityRequest;
pub use messages::{ManageMessagesRequest, MessageInput};
pub use query::QueryConversationContextRequest;
pub use search::SemanticSearchRequest;
pub use session::SessionRequest;
pub use summary::GetStructuredSummaryRequest;
pub use update_context::{
    ContextUpdateItem, EntityItem, RelationItem, UpdateConversationContextRequest,
};
pub use workspace::ManageWorkspaceRequest;
