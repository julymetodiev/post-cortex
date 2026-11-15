// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Documents: the documents modules keep in memory, versioned
//! (ARCHITECTURE sections 4 and 7, modules together). Only the module that
//! declares a kind writes its documents, and only data of its kind's
//! message (W23); anyone who reads the workspace reads them. Each version's title
//! and text is embedded as derived work, unless its kind is not searchable;
//! search finds the latest.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use post_cortex_core::core::document::{
    Document, DocumentCursor, DocumentData, DocumentDraft, DocumentKind, KindDeclaration,
    MAX_DOCUMENT_DATA_BYTES, MAX_DOCUMENT_TEXT_BYTES, content_id,
};
use post_cortex_core::redact::{RedactionCounts, Redactor};
use post_cortex_ports::{
    BlobStore, DocumentKinds, DocumentStore, DocumentWrite, KindsError, LatestQuery,
};
use serde_json::{Map, Value};
use uuid::Uuid;

use crate::capabilities::documents::typed::Typed;

use crate::capabilities::derived::DerivedQueue;
use crate::graph_views::detail::PageCursor;
use crate::memory_system::{Change, DocumentChange, MemoryRuntime};
use crate::pipeline::Embed;

/// Why a document operation failed.
#[derive(Debug, thiserror::Error)]
pub enum DocumentError {
    /// The workspace does not exist.
    #[error("Workspace {0} not found")]
    WorkspaceNotFound(Uuid),
    /// The document does not exist.
    #[error("Document {0} not found")]
    NotFound(Uuid),
    /// The request is malformed, or the data does not fit its kind.
    #[error("Invalid argument: {0}")]
    InvalidArgument(String),
    /// The caller may not write documents of this kind.
    #[error("Permission denied: {0}")]
    Denied(String),
    /// The running modules' document kinds could not be read.
    #[error("Document kinds: {0}")]
    Kinds(#[from] KindsError),
    /// Storage failed.
    #[error("Internal error: {0}")]
    Storage(#[from] crate::memory_system::RuntimeError),
}

/// Who writes: the module the host names as the caller, and where the
/// running modules' kinds are read.
#[derive(Clone, Copy)]
pub struct Writer<'a> {
    /// The calling module (`Scope.caller_module`); none: not a module.
    pub module: Option<&'a str>,
    /// The document kinds of the running modules.
    pub kinds: &'a dyn DocumentKinds,
    /// The reach of the call that writes (docs/connect.md 4a); the modules
    /// that follow the write act within it. None: every workspace.
    pub reach: Option<&'a [Uuid]>,
}

/// The Documents capability service.
#[derive(Clone)]
pub struct Documents {
    rt: Arc<MemoryRuntime>,
    documents: Arc<dyn DocumentStore>,
    blobs: Option<Arc<dyn BlobStore>>,
    derived: DerivedQueue,
    redactor: Arc<Redactor>,
}

impl Documents {
    pub(crate) fn new(
        rt: Arc<MemoryRuntime>,
        documents: Arc<dyn DocumentStore>,
        blobs: Option<Arc<dyn BlobStore>>,
        derived: DerivedQueue,
        redactor: Arc<Redactor>,
    ) -> Self {
        Self {
            rt,
            documents,
            blobs,
            derived,
            redactor,
        }
    }

    /// Write the next version of a document (redacted, within the size
    /// limits, its data fitting its kind) unless it changed since
    /// `expected_version`; a conflict is a normal answer carrying the latest
    /// version. Only the module that declares the kind writes it.
    ///
    /// # Errors
    /// `WorkspaceNotFound`; `Denied` when `writer` is not the kind's module;
    /// `InvalidArgument` for a kind no running module declares, data its
    /// schema refuses, a missing title, a size over a limit, an image
    /// (`pcx.v1.Attachment`) the workspace has no blob of, or an id with
    /// `expected_version` 0 and none without; storage failed.
    #[tracing::instrument(skip_all, fields(kind = %req.kind, expected_version = req.expected_version), name = "post_cortex.put_document")]
    pub async fn put(
        &self,
        req: PutDocument,
        writer: Writer<'_>,
    ) -> Result<PutOutcome, DocumentError> {
        self.require_workspace(req.workspace_id)?;
        let kinds = writer.kinds.kinds().await?;
        let declared = owned_kind(&req.kind, writer.module, &kinds)?;
        let typed = Typed::of(declared)?;
        let id = match (req.id, req.expected_version) {
            (None, 0) => Uuid::new_v4(),
            (Some(id), version) if version > 0 => id,
            (None, _) => {
                return Err(invalid("a change of a document names its id"));
            }
            (Some(_), _) => {
                return Err(invalid("a new document names no id; memory gives it one"));
            }
        };
        let mut counts = RedactionCounts::new();
        let text = self.redactor.redact_str(req.text.trim_end(), &mut counts);
        let mut message = typed.decode(&req.data)?;
        typed::redact(&mut message, &self.redactor, &mut counts);
        if !counts.is_empty() {
            tracing::info!(kind = %req.kind, ?counts, "put_document: redacted sensitive values");
        }
        let named = attachments::named(&message)?;
        attachments::check(self.blobs.as_deref(), req.workspace_id, &named).await?;
        let title = typed.title(&message);
        if title.is_empty() {
            return Err(invalid(&format!(
                "the data has no title {}",
                declared.title_field
            )));
        }
        let data = typed::encode(&message);
        check_size(&data, &text)?;
        let draft = DocumentDraft {
            workspace_id: req.workspace_id,
            id,
            kind: req.kind,
            data,
            title,
            text,
            author: req.author,
            keys: keys::of(&message),
        };
        let write = self
            .documents
            .put_document(draft, req.expected_version)
            .await
            .map_err(storage)?;
        Ok(match write {
            DocumentWrite::Written(document) => {
                self.publish(
                    &document.kind,
                    document.workspace_id,
                    document.id,
                    Some(document.version),
                    document.updated_at,
                    writer.reach.map(<[Uuid]>::to_vec),
                );
                if declared.searchable {
                    self.derived
                        .submit(document.workspace_id, Embed::Document(document.id))
                        .await;
                }
                PutOutcome {
                    written: true,
                    document: Some(document),
                }
            }
            DocumentWrite::Conflict(latest) => PutOutcome {
                written: false,
                document: latest,
            },
        })
    }

    /// The latest version of a document, or `version` when one is named.
    ///
    /// # Errors
    /// `WorkspaceNotFound`, `NotFound`, or storage failed.
    pub async fn get(
        &self,
        workspace_id: Uuid,
        id: Uuid,
        version: Option<u64>,
    ) -> Result<Document, DocumentError> {
        self.require_workspace(workspace_id)?;
        self.documents
            .get_document(workspace_id, id, version)
            .await
            .map_err(storage)?
            .ok_or(DocumentError::NotFound(id))
    }

    /// Up to `limit` versions of a document, newest first.
    ///
    /// # Errors
    /// `WorkspaceNotFound`, `NotFound` for a document without versions, or
    /// storage failed.
    pub async fn history(
        &self,
        workspace_id: Uuid,
        id: Uuid,
        limit: usize,
    ) -> Result<Vec<Document>, DocumentError> {
        self.require_workspace(workspace_id)?;
        let versions = self
            .documents
            .document_history(workspace_id, id, limit)
            .await
            .map_err(storage)?;
        if versions.is_empty() {
            return Err(DocumentError::NotFound(id));
        }
        Ok(versions)
    }

    /// A page of the documents of a kind whose data matches the filter,
    /// the most recently changed first, read by storage alone: a filter is
    /// a lookup of its example's keys (docs/memory.md, document keys and
    /// pages), and the page is checked against it again.
    ///
    /// # Errors
    /// `WorkspaceNotFound`; `InvalidArgument` for a filter of a kind no
    /// running module declares, of another type, or naming a field the type
    /// does not have or one that is not a top-level singular scalar;
    /// storage failed.
    pub async fn list(
        &self,
        req: ListDocuments,
        kinds: &dyn DocumentKinds,
    ) -> Result<DocumentPage, DocumentError> {
        self.require_workspace(req.workspace_id)?;
        let filter = match &req.filter {
            None => None,
            Some(filter) => {
                let typed = Typed::of(declared(&req.kind, &kinds.kinds().await?)?)?;
                let example = typed.decode(&filter.example)?;
                let keys = keys::of_filter(typed.descriptor(), &example, &filter.fields)?;
                Some((typed, example, keys))
            }
        };
        let page = self
            .documents
            .latest_page(LatestQuery {
                workspace_id: req.workspace_id,
                kind: &req.kind,
                keys: filter
                    .as_ref()
                    .map_or(&[][..], |(_, _, keys)| keys.as_slice()),
                after: req.after.as_ref().map(|c| DocumentCursor {
                    at: c.at,
                    id: c.key,
                }),
                limit: req.limit,
            })
            .await
            .map_err(storage)?;
        let mut documents = page.documents;
        if let (Some((typed, example, _)), Some(wanted)) = (&filter, &req.filter) {
            let mut kept = Vec::with_capacity(documents.len());
            for document in documents {
                let data = typed.decode(&document.data)?;
                if pcx_module_api::data_matches(&data, example, &wanted.fields) {
                    kept.push(document);
                }
            }
            documents = kept;
        }
        Ok(DocumentPage {
            documents,
            total: page.total,
            next: page.next.map(|c| PageCursor {
                at: c.at,
                key: c.id,
            }),
        })
    }

    /// The data of each of `documents` as JSON, by the canonical proto
    /// mapping of its kind's message (data written before W23 as it is),
    /// for the MCP tool.
    ///
    /// # Errors
    /// A document of a kind no running module declares, or data that does
    /// not decode as its type.
    pub async fn json_data(
        &self,
        documents: &[Document],
        kinds: &dyn DocumentKinds,
    ) -> Result<Vec<Map<String, Value>>, DocumentError> {
        let kinds = kinds.kinds().await?;
        documents
            .iter()
            .map(|document| match &document.data {
                DocumentData::Json(map) => Ok(map.clone()),
                data => typed::json(&Typed::of(declared(&document.kind, &kinds)?)?.decode(data)?),
            })
            .collect()
    }

    /// Delete a document with every version. Only the module that declares
    /// its kind deletes it.
    ///
    /// # Errors
    /// `WorkspaceNotFound`, `NotFound`, `Denied`, or storage failed.
    pub async fn delete(
        &self,
        workspace_id: Uuid,
        id: Uuid,
        writer: Writer<'_>,
    ) -> Result<(), DocumentError> {
        let latest = self.get(workspace_id, id, None).await?;
        let kinds = writer.kinds.kinds().await?;
        owned_kind(&latest.kind, writer.module, &kinds)?;
        if !self
            .documents
            .delete_document(workspace_id, id)
            .await
            .map_err(storage)?
        {
            return Err(DocumentError::NotFound(id));
        }
        self.rt
            .drop_content_vectors(workspace_id, &[content_id(id)])
            .await;
        self.publish(
            &latest.kind,
            workspace_id,
            id,
            None,
            Utc::now(),
            writer.reach.map(<[Uuid]>::to_vec),
        );
        Ok(())
    }

    fn publish(
        &self,
        kind: &DocumentKind,
        workspace_id: Uuid,
        id: Uuid,
        version: Option<u64>,
        at: DateTime<Utc>,
        reach: Option<Vec<Uuid>>,
    ) {
        self.rt.changes.publish(Change::Document(DocumentChange {
            workspace_id,
            id,
            kind: kind.clone(),
            version,
            at,
            reach,
        }));
    }

    fn require_workspace(&self, id: Uuid) -> Result<(), DocumentError> {
        match self.rt.workspace_manager.get_workspace(&id) {
            Some(_) => Ok(()),
            None => Err(DocumentError::WorkspaceNotFound(id)),
        }
    }
}

/// The declaration of `kind` among `kinds`, if `module` is the module
/// that declares it.
fn owned_kind<'a>(
    kind: &DocumentKind,
    module: Option<&str>,
    kinds: &'a [KindDeclaration],
) -> Result<&'a KindDeclaration, DocumentError> {
    let declared = declared(kind, kinds)?;
    if module != Some(kind.module()) {
        return Err(DocumentError::Denied(format!(
            "only module {} writes documents of kind {kind}",
            kind.module()
        )));
    }
    Ok(declared)
}

/// The declaration of `kind` among `kinds`.
fn declared<'a>(
    kind: &DocumentKind,
    kinds: &'a [KindDeclaration],
) -> Result<&'a KindDeclaration, DocumentError> {
    kinds.iter().find(|d| d.kind == *kind).ok_or_else(|| {
        invalid(&format!(
            "no running module declares the document kind {kind}"
        ))
    })
}

fn check_size(data: &DocumentData, text: &str) -> Result<(), DocumentError> {
    let data_bytes = match data {
        DocumentData::Message { bytes, .. } => bytes.len(),
        DocumentData::Json(map) => serde_json::to_vec(map)
            .map_err(|e| invalid(&format!("the data does not encode: {e}")))?
            .len(),
    };
    if data_bytes > MAX_DOCUMENT_DATA_BYTES {
        return Err(invalid(&format!(
            "the data is {data_bytes} bytes, the limit is {MAX_DOCUMENT_DATA_BYTES}"
        )));
    }
    if text.len() > MAX_DOCUMENT_TEXT_BYTES {
        return Err(invalid(&format!(
            "the text is {} bytes, the limit is {MAX_DOCUMENT_TEXT_BYTES}",
            text.len()
        )));
    }
    Ok(())
}

fn invalid(reason: &str) -> DocumentError {
    DocumentError::InvalidArgument(reason.to_string())
}

/// The one mapping of a storage port error to this capability's error.
fn storage(e: post_cortex_ports::PortError) -> DocumentError {
    DocumentError::Storage(e.into())
}

mod attachments;
mod keys;
mod migrate;
mod requests;
mod typed;

pub use migrate::Migrated;
pub use requests::{DocumentFilter, DocumentPage, ListDocuments, PutDocument, PutOutcome};

#[cfg(test)]
mod tests;
