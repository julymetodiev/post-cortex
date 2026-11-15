// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Documents of modules (ARCHITECTURE section 7, modules together).
//!
//! A module keeps its own state in memory as documents of the kinds its
//! manifest declares (`board/task`). A document lives in a workspace and
//! every change is a new version; writers name the version they read, so
//! two writers never overwrite each other. Every version is kept.

use std::fmt;
use std::str::FromStr;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use uuid::Uuid;

use crate::core::wire::UnknownVariant;

/// Largest data of a document, in bytes of its encoded message.
pub const MAX_DOCUMENT_DATA_BYTES: usize = 64 * 1024;

/// Longest text of a document, in bytes.
pub const MAX_DOCUMENT_TEXT_BYTES: usize = 32 * 1024;

/// A document kind as the running module that owns it declares it (W23):
/// the message of its data, in the module's descriptors, and the field of
/// it that is its title. The host checked both when it loaded the module.
#[derive(Clone, Debug, PartialEq)]
pub struct KindDeclaration {
    /// The kind.
    pub kind: DocumentKind,
    /// The full name of the message of its data.
    pub type_name: String,
    /// The string field of that message that is its title.
    pub title_field: String,
    /// Whether its documents are embedded, so search finds them.
    pub searchable: bool,
    /// The module's encoded `FileDescriptorSet`, which has the message.
    pub descriptors: Vec<u8>,
}

/// The data of a document: a message of its kind's type (W23), or the
/// JSON a version written before W23 holds until the migration converts
/// it (export format 1.7.0).
#[derive(Clone, Debug, PartialEq)]
pub enum DocumentData {
    /// A message, encoded.
    Message {
        /// Its full name.
        type_name: String,
        /// Its encoding.
        bytes: Vec<u8>,
    },
    /// JSON written before W23.
    Json(Map<String, Value>),
}

/// The key of a message's type in the stored JSON of [`DocumentData`] (as
/// in the JSON of a `google.protobuf.Any`); a JSON object without it is
/// data written before W23.
const TYPE_KEY: &str = "@type";

/// The key of a message's encoding (base64) in the stored JSON.
const BYTES_KEY: &str = "bytes";

impl Serialize for DocumentData {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Message { type_name, bytes } => {
                let mut map = Map::new();
                map.insert(TYPE_KEY.to_string(), Value::from(type_name.as_str()));
                map.insert(BYTES_KEY.to_string(), Value::from(BASE64.encode(bytes)));
                map.serialize(serializer)
            }
            Self::Json(map) => map.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for DocumentData {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let map = Map::<String, Value>::deserialize(deserializer)?;
        let Some(type_name) = map.get(TYPE_KEY) else {
            return Ok(Self::Json(map));
        };
        let (Some(type_name), Some(Value::String(bytes)), 2) =
            (type_name.as_str(), map.get(BYTES_KEY), map.len())
        else {
            return Err(serde::de::Error::custom(format!(
                "a document's message data has only {TYPE_KEY} and {BYTES_KEY}"
            )));
        };
        let bytes = BASE64.decode(bytes).map_err(serde::de::Error::custom)?;
        Ok(Self::Message {
            type_name: type_name.to_string(),
            bytes,
        })
    }
}

/// A document kind: the module that owns it and the kind's name in that
/// module, written `<module>/<name>` (`board/task`). Neither part is empty
/// or has a `/`, and the name has no `:` (storage keys end with it). The
/// characters a manifest may use in a name are the module API's to check.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct DocumentKind {
    module: String,
    name: String,
}

impl DocumentKind {
    /// The kind `name` of `module`, if both are well formed.
    pub fn new(module: &str, name: &str) -> Result<Self, UnknownVariant> {
        let refused = || UnknownVariant {
            kind: "document kind",
            value: format!("{module}/{name}"),
        };
        let module_ok = !module.is_empty() && !module.contains('/');
        let name_ok = !name.is_empty() && !name.contains(['/', ':']);
        if module_ok && name_ok {
            Ok(Self {
                module: module.to_string(),
                name: name.to_string(),
            })
        } else {
            Err(refused())
        }
    }

    /// The id of the module that owns the kind.
    pub fn module(&self) -> &str {
        &self.module
    }

    /// The kind's name in its module.
    pub fn name(&self) -> &str {
        &self.name
    }
}

impl fmt::Display for DocumentKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.module, self.name)
    }
}

/// Strict inverse of [`DocumentKind`]'s `Display`.
impl FromStr for DocumentKind {
    type Err = UnknownVariant;

    fn from_str(kind: &str) -> Result<Self, Self::Err> {
        let (module, name) = kind.split_once('/').ok_or_else(|| UnknownVariant {
            kind: "document kind",
            value: kind.to_string(),
        })?;
        Self::new(module, name)
    }
}

impl TryFrom<String> for DocumentKind {
    type Error = UnknownVariant;

    fn try_from(kind: String) -> Result<Self, Self::Error> {
        kind.parse()
    }
}

impl From<DocumentKind> for String {
    fn from(kind: DocumentKind) -> Self {
        kind.to_string()
    }
}

/// The search-index content id of document `id`: `doc:<id>`.
pub fn content_id(id: Uuid) -> String {
    format!("doc:{id}")
}

/// The document of a search-index content id made by [`content_id`].
pub fn of_content_id(content_id: &str) -> Option<Uuid> {
    content_id
        .strip_prefix("doc:")
        .and_then(|id| Uuid::parse_str(id).ok())
}

/// One version of a document.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Document {
    /// The workspace it lives in.
    pub workspace_id: Uuid,
    /// Its id, given by memory at its first version.
    pub id: Uuid,
    /// Its kind; the same in every version.
    pub kind: DocumentKind,
    /// 1 for the first version, one more for every change.
    pub version: u64,
    /// Its data, checked against its kind's type when written.
    pub data: DocumentData,
    /// Its title: the data field its kind names as the title.
    pub title: String,
    /// Its text, at most [`MAX_DOCUMENT_TEXT_BYTES`].
    pub text: String,
    /// When this version was written.
    pub updated_at: DateTime<Utc>,
    /// Who wrote this version, when known.
    pub author: Option<String>,
}

/// A key of a document's latest version (docs/memory.md, document keys):
/// a top-level scalar field of its data, by the field's number and the
/// first 16 bytes of the SHA-256 of its value, written
/// `<number>:<32 hex digits>`. Memory makes them; storage finds documents
/// by them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct DocumentKey {
    field: u32,
    digest: [u8; 16],
}

impl DocumentKey {
    /// The key of field `field` whose value hashes to `digest`.
    pub fn new(field: u32, digest: [u8; 16]) -> Self {
        Self { field, digest }
    }
}

impl fmt::Display for DocumentKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:", self.field)?;
        self.digest.iter().try_for_each(|b| write!(f, "{b:02x}"))
    }
}

/// Strict inverse of [`DocumentKey`]'s `Display`.
impl FromStr for DocumentKey {
    type Err = UnknownVariant;

    fn from_str(key: &str) -> Result<Self, Self::Err> {
        let refused = || UnknownVariant {
            kind: "document key",
            value: key.to_string(),
        };
        let (number, hex) = key.split_once(':').ok_or_else(refused)?;
        let field: u32 = number.parse().map_err(|_| refused())?;
        if field.to_string() != number || hex.len() != 32 {
            return Err(refused());
        }
        let mut digest = [0u8; 16];
        for (i, byte) in digest.iter_mut().enumerate() {
            let pair = hex.get(i * 2..i * 2 + 2).ok_or_else(refused)?;
            if pair.bytes().any(|b| b.is_ascii_uppercase()) {
                return Err(refused());
            }
            *byte = u8::from_str_radix(pair, 16).map_err(|_| refused())?;
        }
        Ok(Self { field, digest })
    }
}

impl TryFrom<String> for DocumentKey {
    type Error = UnknownVariant;

    fn try_from(key: String) -> Result<Self, Self::Error> {
        key.parse()
    }
}

impl From<DocumentKey> for String {
    fn from(key: DocumentKey) -> Self {
        key.to_string()
    }
}

/// Where a document stands in a list of the latest versions, the most
/// recently changed first: its latest version's time, then its id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DocumentCursor {
    /// When its latest version was written.
    pub at: DateTime<Utc>,
    /// The document.
    pub id: Uuid,
}

impl DocumentCursor {
    /// Where `document` stands.
    pub fn of(document: &Document) -> Self {
        Self {
            at: document.updated_at,
            id: document.id,
        }
    }
}

/// The content of a document's next version: what a writer gives.
#[derive(Clone, Debug, PartialEq)]
pub struct DocumentDraft {
    /// The workspace it lives in.
    pub workspace_id: Uuid,
    /// The document.
    pub id: Uuid,
    /// Its kind.
    pub kind: DocumentKind,
    /// Its data.
    pub data: DocumentData,
    /// Its title.
    pub title: String,
    /// Its text.
    pub text: String,
    /// Who writes this version.
    pub author: Option<String>,
    /// The keys of its data (docs/memory.md, document keys).
    pub keys: Vec<DocumentKey>,
}

impl DocumentDraft {
    /// This draft as `version`, written at `at`, and its keys.
    pub fn into_version(self, version: u64, at: DateTime<Utc>) -> (Document, Vec<DocumentKey>) {
        let document = Document {
            workspace_id: self.workspace_id,
            id: self.id,
            kind: self.kind,
            version,
            data: self.data,
            title: self.title,
            text: self.text,
            updated_at: at,
            author: self.author,
        };
        (document, self.keys)
    }
}

#[cfg(test)]
mod tests;
