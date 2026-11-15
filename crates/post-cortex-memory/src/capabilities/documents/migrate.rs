// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The migration at start (docs/memory.md, document keys and pages): each
//! unkeyed latest version gets its keys. A document with a version whose
//! data is JSON (written before W23, export format 1.7.0) first has every
//! version read into its kind's message and stored again as it was, with
//! its number, time and author. Only unkeyed documents are read, so a
//! start after the first reads none.

use post_cortex_core::core::document::{Document, DocumentData, KindDeclaration};
use post_cortex_ports::DocumentKinds;

use crate::capabilities::documents::typed::{self, Typed};
use crate::capabilities::documents::{DocumentError, Documents, declared, keys, storage};

/// What a migration did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Migrated {
    /// Documents given their keys.
    pub keyed: usize,
    /// Of those, documents whose versions were converted from JSON.
    pub converted: usize,
    /// Documents left unkeyed: no running module declares their kind.
    pub undeclared: usize,
    /// Documents left unkeyed: a version does not read as their kind's
    /// message (each logged).
    pub unreadable: usize,
}

impl Documents {
    /// Key every unkeyed latest version, converting a document with JSON
    /// data to its kind's message first; a document whose kind no running
    /// module declares is left for a later start, one a version of which
    /// does not read as the message is left as it is and logged. Run by
    /// the composition once the modules run; until then a page with keys
    /// holds the unkeyed documents, so none is missed.
    ///
    /// # Errors
    /// The kinds cannot be read, or storage failed.
    pub async fn migrate(&self, kinds: &dyn DocumentKinds) -> Result<Migrated, DocumentError> {
        let kinds = kinds.kinds().await?;
        let mut migrated = Migrated::default();
        for workspace in self.rt.workspace_manager.list_workspaces() {
            let unkeyed = self
                .documents
                .unkeyed_documents(workspace.id)
                .await
                .map_err(storage)?;
            for document in unkeyed {
                let Ok(declared) = declared(&document.kind, &kinds) else {
                    tracing::warn!(kind = %document.kind, id = %document.id, "documents: no running module declares the kind; left unkeyed");
                    migrated.undeclared += 1;
                    continue;
                };
                match self.key(declared, &document).await {
                    Ok(converted) => {
                        migrated.keyed += 1;
                        migrated.converted += usize::from(converted);
                    }
                    Err(DocumentError::InvalidArgument(reason)) => {
                        tracing::warn!(kind = %document.kind, id = %document.id, error = %reason, "documents: a version does not read as its kind's message; left unkeyed");
                        migrated.unreadable += 1;
                    }
                    Err(e) => return Err(e),
                }
            }
        }
        if migrated != Migrated::default() {
            tracing::info!(?migrated, "documents: unkeyed documents migrated");
        }
        Ok(migrated)
    }

    /// Give `latest`, unkeyed, its keys: whether its versions were
    /// converted from JSON first.
    async fn key(
        &self,
        declared: &KindDeclaration,
        latest: &Document,
    ) -> Result<bool, DocumentError> {
        let typed = Typed::of(declared)?;
        let (ws, id) = (latest.workspace_id, latest.id);
        let versions = self
            .documents
            .document_history(ws, id, usize::MAX)
            .await
            .map_err(storage)?;
        if !versions
            .iter()
            .any(|v| matches!(v.data, DocumentData::Json(_)))
        {
            let message = typed.decode(&latest.data)?;
            // False: a write since keyed its own version.
            self.documents
                .set_document_keys(ws, id, latest.version, keys::of(&message))
                .await
                .map_err(storage)?;
            return Ok(false);
        }
        let converted = convert(&typed, versions)?;
        let newest = converted
            .iter()
            .max_by_key(|v| v.version)
            .map(|v| typed.decode(&v.data))
            .transpose()?;
        self.documents
            .restore_document_versions(ws, id, converted, true, newest.as_ref().map(keys::of))
            .await
            .map_err(storage)?;
        Ok(true)
    }
}

/// `versions` with their data as `typed`'s message.
fn convert(typed: &Typed, versions: Vec<Document>) -> Result<Vec<Document>, DocumentError> {
    versions
        .into_iter()
        .map(|version| {
            let message = typed.decode(&version.data)?;
            Ok(Document {
                data: typed::encode(&message),
                ..version
            })
        })
        .collect()
}
