// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A kind's documents read and written as its message (W23).

use pcx_module_api::{ModuleError, ModuleErrorKind};
use post_cortex_proto::pb::{Document, DocumentFilter};
use prost::{Message, Name};

use crate::Documents;

/// Documents read per page.
const PAGE: u32 = 100;

/// A document read as its kind's message: the version as memory answers
/// it, and its data decoded.
#[derive(Debug, Clone, PartialEq)]
pub struct Item<T> {
    /// The version.
    pub document: Document,
    /// Its data.
    pub data: T,
}

/// A page of a kind's documents, as its message, and where the next
/// starts.
#[derive(Debug, Clone, PartialEq)]
pub struct Paged<T> {
    /// The page, the most recently changed first.
    pub items: Vec<Item<T>>,
    /// The cursor of the next page; none on the last.
    pub next: Option<String>,
}

/// The documents of one kind, as its message.
pub struct Typed<'a> {
    docs: &'a dyn Documents,
    kind: String,
    /// The kind's name in its module, as messages name it.
    noun: String,
}

impl<'a> Typed<'a> {
    /// The documents of `kind` (`<module id>/<name>`) in `docs`.
    #[must_use]
    pub fn new(docs: &'a dyn Documents, kind: impl Into<String>) -> Self {
        let kind = kind.into();
        let noun = kind
            .split_once('/')
            .map_or(kind.as_str(), |(_, name)| name)
            .to_string();
        Self { docs, kind, noun }
    }

    /// Document `id`, none when there is none.
    ///
    /// # Errors
    /// Its data is not a `T`; memory failed.
    pub async fn find<T: Message + Name + Default>(
        &self,
        id: &str,
    ) -> Result<Option<Item<T>>, ModuleError> {
        self.docs
            .get(&self.kind, id)
            .await?
            .map(|document| self.item(document))
            .transpose()
    }

    /// Version `version` of document `id`, none when there is none.
    ///
    /// # Errors
    /// Its data is not a `T`; memory failed.
    pub async fn find_version<T: Message + Name + Default>(
        &self,
        id: &str,
        version: u64,
    ) -> Result<Option<Item<T>>, ModuleError> {
        self.docs
            .version(&self.kind, id, version)
            .await?
            .map(|document| self.item(document))
            .transpose()
    }

    /// Document `id`.
    ///
    /// # Errors
    /// `NotFound` when there is none; as [`Typed::find`].
    pub async fn get<T: Message + Name + Default>(&self, id: &str) -> Result<Item<T>, ModuleError> {
        self.find(id).await?.ok_or_else(|| self.not_found(id))
    }

    /// The documents whose data has `fields` equal to `example`'s (none:
    /// all of them), the most recently changed first.
    ///
    /// # Errors
    /// As [`Typed::find`].
    pub async fn all<T: Message + Name + Default>(
        &self,
        matching: Option<(&T, &[&str])>,
    ) -> Result<Vec<Item<T>>, ModuleError> {
        let mut all = Vec::new();
        self.scan(matching, |item| {
            all.push(item);
            true
        })
        .await?;
        Ok(all)
    }

    /// The documents whose data has `fields` equal to `example`'s (none:
    /// all of them), the most recently changed first, each given to
    /// `visit` until it answers false: read page by page, only as far as
    /// it goes (docs/host.md, pages in a module).
    ///
    /// # Errors
    /// As [`Typed::find`].
    pub async fn scan<T: Message + Name + Default>(
        &self,
        matching: Option<(&T, &[&str])>,
        mut visit: impl FnMut(Item<T>) -> bool + Send,
    ) -> Result<(), ModuleError> {
        let filter = self.filter(matching)?;
        let mut cursor: Option<String> = None;
        loop {
            let page = self
                .docs
                .page(&self.kind, filter.clone(), PAGE, cursor.as_deref())
                .await?;
            for document in page.documents {
                if !visit(self.item(document)?) {
                    return Ok(());
                }
            }
            match page.next {
                Some(next) => cursor = Some(next),
                None => return Ok(()),
            }
        }
    }

    /// At most `limit` of the documents `matching` names that `keep`
    /// keeps, after document `before` (none: from the first), the most
    /// recently changed first, and whether more follow: a page of a list
    /// whose reader names the last one it has.
    ///
    /// # Errors
    /// `NotFound` when `before` is none of them; as [`Typed::find`].
    pub async fn after<T: Message + Name + Default>(
        &self,
        matching: Option<(&T, &[&str])>,
        before: Option<&str>,
        limit: usize,
        keep: impl Fn(&Item<T>) -> bool + Send + Sync,
    ) -> Result<(Vec<Item<T>>, bool), ModuleError> {
        let (mut found, mut more, mut past) = (Vec::new(), false, before.is_none());
        self.scan(matching, |item| {
            if !past {
                past = before == Some(item.document.id.as_str());
                return true;
            }
            if !keep(&item) {
                return true;
            }
            if found.len() == limit {
                more = true;
                return false;
            }
            found.push(item);
            true
        })
        .await?;
        match before {
            Some(id) if !past => Err(self.not_found(id)),
            _ => Ok((found, more)),
        }
    }

    /// A page of at most `limit` of the documents whose data has `fields`
    /// equal to `example`'s (none: all of them), the most recently changed
    /// first, after `cursor` (none: the first page). A page may hold fewer
    /// than `limit` while more follow.
    ///
    /// # Errors
    /// As [`Typed::find`].
    pub async fn page<T: Message + Name + Default>(
        &self,
        matching: Option<(&T, &[&str])>,
        limit: u32,
        cursor: Option<&str>,
    ) -> Result<Paged<T>, ModuleError> {
        let page = self
            .docs
            .page(&self.kind, self.filter(matching)?, limit, cursor)
            .await?;
        Ok(Paged {
            items: page
                .documents
                .into_iter()
                .map(|document| self.item(document))
                .collect::<Result<_, _>>()?,
            next: page.next,
        })
    }

    /// The filter of `matching`.
    fn filter<T: Message + Name>(
        &self,
        matching: Option<(&T, &[&str])>,
    ) -> Result<Option<DocumentFilter>, ModuleError> {
        matching
            .map(|(example, fields)| {
                Ok(DocumentFilter {
                    example: Some(self.any(example)?),
                    fields: fields.iter().map(|f| (*f).to_string()).collect(),
                })
            })
            .transpose()
    }

    /// A new document of `value` with `text`.
    ///
    /// # Errors
    /// Memory refused or failed.
    pub async fn create<T: Message + Name + Default>(
        &self,
        value: &T,
        text: &str,
    ) -> Result<Item<T>, ModuleError> {
        let put = self
            .docs
            .put(&self.kind, None, 0, self.any(value)?, text)
            .await?;
        match (put.written, put.document) {
            (true, Some(document)) => self.item(document),
            _ => Err(ModuleError::new(
                ModuleErrorKind::Internal,
                format!("a new {} was not written", self.noun),
            )),
        }
    }

    /// The next version of `id`, `value` with `text`, if its latest is
    /// `version`.
    ///
    /// # Errors
    /// `InvalidArgument` for version 0 (a document's first version is 1:
    /// 0 names no version read); `Conflict` when it changed since, naming
    /// its latest version; `NotFound` when it is gone; memory refused or
    /// failed.
    pub async fn change<T: Message + Name + Default>(
        &self,
        id: &str,
        version: u64,
        value: &T,
        text: &str,
    ) -> Result<Item<T>, ModuleError> {
        if version == 0 {
            return Err(ModuleError::new(
                ModuleErrorKind::InvalidArgument,
                format!(
                    "a change of {} {id} names the version you read (version 0 is none); read it and pass its version",
                    self.noun
                ),
            ));
        }
        let put = self
            .docs
            .put(&self.kind, Some(id), version, self.any(value)?, text)
            .await?;
        match (put.written, put.document) {
            (true, Some(document)) => self.item(document),
            (false, Some(latest)) => Err(ModuleError::new(
                ModuleErrorKind::Conflict,
                format!(
                    "{} {id} changed since version {version}; read version {} and decide again",
                    self.noun, latest.version
                ),
            )),
            _ => Err(self.not_found(id)),
        }
    }

    /// Delete document `id` with its history.
    ///
    /// # Errors
    /// `NotFound` when there is none; memory failed.
    pub async fn delete(&self, id: &str) -> Result<(), ModuleError> {
        if self.docs.delete(&self.kind, id).await? {
            Ok(())
        } else {
            Err(self.not_found(id))
        }
    }

    /// The answer to a call that names document `id`, which there is not.
    #[must_use]
    pub fn not_found(&self, id: &str) -> ModuleError {
        ModuleError::new(ModuleErrorKind::NotFound, format!("no {} {id}", self.noun))
    }

    /// `document` with its data as `T`.
    fn item<T: Message + Name + Default>(
        &self,
        document: Document,
    ) -> Result<Item<T>, ModuleError> {
        let unreadable = |reason: String| {
            ModuleError::new(
                ModuleErrorKind::Internal,
                format!("{} {} does not read: {reason}", self.noun, document.id),
            )
        };
        let data = document
            .data
            .as_ref()
            .ok_or_else(|| unreadable("it has no data".to_string()))?
            .to_msg::<T>()
            .map_err(|e| unreadable(e.to_string()))?;
        Ok(Item { document, data })
    }

    /// `value` as a document's data.
    fn any<T: Message + Name>(&self, value: &T) -> Result<prost_types::Any, ModuleError> {
        prost_types::Any::from_msg(value).map_err(|e| {
            ModuleError::new(
                ModuleErrorKind::Internal,
                format!("a {} does not encode: {e}", self.noun),
            )
        })
    }
}

#[cfg(test)]
mod tests;
