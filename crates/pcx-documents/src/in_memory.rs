// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! [`Documents`] held in memory, versioned as memory versions them: ids
//! given at the first write, a write from a stale version a conflict, a
//! filter as memory reads it (its data decoded with the module's
//! descriptors).

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use pcx_module_api::{ModuleError, ModuleErrorKind};
use post_cortex_proto::pb::{Document, DocumentFilter, PutDocumentResponse};
use prost_reflect::{DescriptorPool, DynamicMessage};

use crate::{Documents, Page};

/// When every version is written unless [`InMemoryDocuments::written_at`]
/// says otherwise (2026-10-25T10:00:00Z).
const WRITTEN_AT: i64 = 1_792_922_400;

/// Every document's versions, oldest first, by kind and id.
type Versions = BTreeMap<(String, String), Vec<Document>>;

/// Documents in memory, each version written by `author`.
pub struct InMemoryDocuments {
    author: Option<String>,
    /// When each version is written, in Unix seconds.
    at: i64,
    /// The messages of the module's documents.
    pool: DescriptorPool,
    next: Arc<Mutex<u64>>,
    /// (kind, id) -> versions, oldest first.
    versions: Arc<Mutex<Versions>>,
}

impl InMemoryDocuments {
    /// No documents of a module whose messages `descriptors` (an encoded
    /// `FileDescriptorSet`) has; every version is written by `author`.
    ///
    /// # Errors
    /// `Invalid` when the descriptors do not read.
    pub fn new(author: Option<&str>, descriptors: &[u8]) -> Result<Self, ModuleError> {
        let pool = pcx_module_api::descriptors(descriptors)
            .map_err(|e| ModuleError::new(ModuleErrorKind::InvalidArgument, e))?;
        Ok(Self {
            author: author.map(str::to_string),
            at: WRITTEN_AT,
            pool,
            next: Arc::new(Mutex::new(0)),
            versions: Arc::new(Mutex::new(BTreeMap::new())),
        })
    }

    /// The same documents, each version from here on written by `author`
    /// (another caller of the one store).
    #[must_use]
    pub fn as_author(&self, author: &str) -> Self {
        Self {
            author: Some(author.to_string()),
            at: self.at,
            pool: self.pool.clone(),
            next: Arc::clone(&self.next),
            versions: Arc::clone(&self.versions),
        }
    }

    /// The same documents, each version from here on written at `at`: the
    /// time of the test's clock, as memory writes at the time of its own.
    #[must_use]
    pub fn written_at(self, at: DateTime<Utc>) -> Self {
        Self {
            at: at.timestamp(),
            ..self
        }
    }

    /// `any` decoded as the message it names.
    fn decode(&self, any: Option<&prost_types::Any>) -> Result<DynamicMessage, ModuleError> {
        let invalid = |reason: String| ModuleError::new(ModuleErrorKind::InvalidArgument, reason);
        let any = any.ok_or_else(|| invalid("no data".to_string()))?;
        let name = any
            .type_url
            .rsplit_once('/')
            .map_or(any.type_url.as_str(), |(_, name)| name);
        let message = self
            .pool
            .get_message_by_name(name)
            .ok_or_else(|| invalid(format!("no message {name}")))?;
        DynamicMessage::decode(message, any.value.as_slice())
            .map_err(|e| invalid(format!("not a {name}: {e}")))
    }

    /// Whether `document` is kept by `filter`.
    fn kept(&self, document: &Document, filter: &DocumentFilter) -> Result<bool, ModuleError> {
        Ok(pcx_module_api::data_matches(
            &self.decode(document.data.as_ref())?,
            &self.decode(filter.example.as_ref())?,
            &filter.fields,
        ))
    }
}

#[async_trait]
impl Documents for InMemoryDocuments {
    async fn delete(&self, kind: &str, id: &str) -> Result<bool, ModuleError> {
        Ok(self
            .versions
            .lock()
            .remove(&(kind.to_string(), id.to_string()))
            .is_some())
    }

    async fn get(&self, kind: &str, id: &str) -> Result<Option<Document>, ModuleError> {
        Ok(self
            .versions
            .lock()
            .get(&(kind.to_string(), id.to_string()))
            .and_then(|v| v.last().cloned()))
    }

    async fn version(
        &self,
        kind: &str,
        id: &str,
        version: u64,
    ) -> Result<Option<Document>, ModuleError> {
        Ok(self
            .versions
            .lock()
            .get(&(kind.to_string(), id.to_string()))
            .and_then(|v| v.iter().find(|d| d.version == version).cloned()))
    }

    async fn put(
        &self,
        kind: &str,
        id: Option<&str>,
        expected: u64,
        data: prost_types::Any,
        text: &str,
    ) -> Result<PutDocumentResponse, ModuleError> {
        // The data is its kind's message, as memory checks.
        self.decode(Some(&data))?;
        let mut versions = self.versions.lock();
        // As memory: a new document gets its id, a change names one.
        let id = match id {
            Some(_) if expected == 0 => {
                return Err(ModuleError::new(
                    ModuleErrorKind::InvalidArgument,
                    "a new document names no id; memory gives it one",
                ));
            }
            Some(id) => id.to_string(),
            None => {
                let mut next = self.next.lock();
                *next += 1;
                format!("{kind}-{next}")
            }
        };
        let all = versions.entry((kind.to_string(), id.clone())).or_default();
        let latest = all.last().map_or(0, |d| d.version);
        if latest != expected {
            return Ok(PutDocumentResponse {
                written: false,
                document: all.last().cloned(),
            });
        }
        let document = Document {
            workspace_id: String::new(),
            id,
            kind: kind.to_string(),
            version: latest + 1,
            data: Some(data),
            title: String::new(),
            text: text.to_string(),
            updated_at: Some(prost_types::Timestamp {
                seconds: self.at,
                nanos: 0,
            }),
            author: self.author.clone(),
        };
        all.push(document.clone());
        Ok(PutDocumentResponse {
            written: true,
            document: Some(document),
        })
    }

    async fn page(
        &self,
        kind: &str,
        filter: Option<DocumentFilter>,
        limit: u32,
        cursor: Option<&str>,
    ) -> Result<Page, ModuleError> {
        let latest: Vec<Document> = self
            .versions
            .lock()
            .iter()
            .filter(|((k, _), _)| k == kind)
            .filter_map(|(_, v)| v.last().cloned())
            .collect();
        let mut found = Vec::with_capacity(latest.len());
        for document in latest {
            let keep = match &filter {
                Some(filter) => self.kept(&document, filter)?,
                None => true,
            };
            if keep {
                found.push(document);
            }
        }
        // Ids are given in order: the newest last.
        found.reverse();
        // The cursor is the last id of the page before.
        let start = match cursor {
            Some(after) => found
                .iter()
                .position(|d| d.id == after)
                .map_or(found.len(), |at| at + 1),
            None => 0,
        };
        let mut rest = found.into_iter().skip(start);
        let documents: Vec<Document> = rest.by_ref().take(limit as usize).collect();
        let next = match rest.next() {
            Some(_) => documents.last().map(|d| d.id.clone()),
            None => None,
        };
        Ok(Page { documents, next })
    }
}
