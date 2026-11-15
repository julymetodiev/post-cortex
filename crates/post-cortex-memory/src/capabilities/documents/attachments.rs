// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The blobs a document's data names (W24): every `pcx.v1.Attachment` in
//! it, at any depth, read by its kind's descriptor. A document's write is
//! refused when its workspace lacks one; a blob no version names is swept.

use post_cortex_core::core::blob::BlobId;
use post_cortex_ports::{BlobStore, DocumentKinds};
use prost_reflect::{DynamicMessage, ReflectMessage, Value as Field};
use uuid::Uuid;

use crate::capabilities::documents::typed::Typed;
use crate::capabilities::documents::{DocumentError, Documents, declared, storage};

/// The message that names a blob.
const ATTACHMENT: &str = "pcx.v1.Attachment";

/// The blobs `message` names, each once, in the order found.
///
/// # Errors
/// `InvalidArgument` for an attachment whose SHA-256 is not one.
pub fn named(message: &DynamicMessage) -> Result<Vec<BlobId>, DocumentError> {
    let mut found = Vec::new();
    walk(message, &mut found)?;
    Ok(found)
}

fn walk(message: &DynamicMessage, found: &mut Vec<BlobId>) -> Result<(), DocumentError> {
    if message.descriptor().full_name() == ATTACHMENT {
        let sha256 = match message.get_field_by_name("sha256").as_deref() {
            Some(Field::String(sha256)) => sha256.clone(),
            _ => String::new(),
        };
        let id: BlobId = sha256.parse().map_err(|e| {
            DocumentError::InvalidArgument(format!("an attachment of the data: {e}"))
        })?;
        if !found.contains(&id) {
            found.push(id);
        }
        return Ok(());
    }
    for (_, value) in message.fields() {
        walk_value(value, found)?;
    }
    Ok(())
}

fn walk_value(value: &Field, found: &mut Vec<BlobId>) -> Result<(), DocumentError> {
    match value {
        Field::Message(message) => walk(message, found),
        Field::List(items) => items.iter().try_for_each(|item| walk_value(item, found)),
        Field::Map(entries) => entries
            .values()
            .try_for_each(|entry| walk_value(entry, found)),
        _ => Ok(()),
    }
}

/// Whether `blobs` keeps every blob of `named` in `workspace_id`.
///
/// # Errors
/// `InvalidArgument` for a blob the workspace does not have, or for any
/// blob when no blob store is configured; the blob store failed.
pub async fn check(
    blobs: Option<&dyn BlobStore>,
    workspace_id: Uuid,
    named: &[BlobId],
) -> Result<(), DocumentError> {
    if named.is_empty() {
        return Ok(());
    }
    let Some(blobs) = blobs else {
        return Err(DocumentError::InvalidArgument(
            "the data names images, and no blob store is configured".to_string(),
        ));
    };
    for id in named {
        if blobs
            .stat_blob(workspace_id, id)
            .await
            .map_err(storage)?
            .is_none()
        {
            return Err(DocumentError::InvalidArgument(format!(
                "the workspace has no blob {id}; put it first"
            )));
        }
    }
    Ok(())
}

impl Documents {
    /// The blobs some version of a document of `workspace_id` names; none
    /// when a document's data does not read (its kind is not declared by a
    /// running module), so what it names is not known.
    ///
    /// # Errors
    /// `WorkspaceNotFound`; the kinds could not be read; storage failed.
    pub async fn named_blobs(
        &self,
        workspace_id: Uuid,
        kinds: &dyn DocumentKinds,
    ) -> Result<Option<Vec<BlobId>>, DocumentError> {
        self.require_workspace(workspace_id)?;
        let kinds = kinds.kinds().await?;
        let latest = self
            .documents
            .latest_documents(workspace_id, None)
            .await
            .map_err(storage)?;
        let mut found = Vec::new();
        for document in latest {
            let Ok(declared) = declared(&document.kind, &kinds) else {
                return Ok(None);
            };
            let typed = Typed::of(declared)?;
            // Its latest version's number is how many it has.
            let every = usize::try_from(document.version).map_err(|e| {
                DocumentError::InvalidArgument(format!("document {}: {e}", document.id))
            })?;
            let versions = self
                .documents
                .document_history(workspace_id, document.id, every)
                .await
                .map_err(storage)?;
            for version in versions {
                walk(&typed.decode(&version.data)?, &mut found)?;
            }
        }
        Ok(Some(found))
    }
}
