// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Blobs (ARCHITECTURE section 4, Blobs; W24): the bytes of an image a
//! document names, kept by their SHA-256 in a workspace. Images (PNG,
//! JPEG, WebP, GIF), known by their first bytes, not their name, and a
//! module's messages (`application/x-protobuf`, as their putter declares
//! them: a workflow step's large output), at most [`MAX_BLOB_BYTES`], and a
//! dynamic module's bundle (`application/x-tar`, as its putter declares it), at
//! most [`MAX_BUNDLE_BYTES`].

use std::fmt;

use chrono::{DateTime, Utc};

/// Largest blob, in bytes.
pub const MAX_BLOB_BYTES: usize = 10 * 1024 * 1024;

/// Largest dynamic module's bundle, in bytes (docs/modules/dynamic.md).
pub const MAX_BUNDLE_BYTES: usize = 16 * 1024 * 1024;

/// A blob's id: its SHA-256 in lowercase hex.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BlobId(String);

impl BlobId {
    /// The id of `digest`, a SHA-256.
    #[must_use]
    pub fn of_digest(digest: &[u8; 32]) -> Self {
        Self(digest.iter().map(|b| format!("{b:02x}")).collect())
    }

    /// The hex of the id.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for BlobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Why a blob's id or bytes are refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BlobRefused {
    /// The id is not 64 lowercase hex digits.
    #[error("{0} is not a SHA-256 in lowercase hex")]
    Id(String),
    /// No bytes.
    #[error("the blob is empty")]
    Empty,
    /// Over the limit.
    #[error("the blob is {size} bytes, the limit is {limit}")]
    TooLarge {
        /// Its size.
        size: usize,
        /// The limit of its media type.
        limit: usize,
    },
    /// Its first bytes are no PNG, JPEG, WebP or GIF.
    #[error("the blob is not a PNG, JPEG, WebP or GIF image")]
    NotAnImage,
    /// A media type that is none of the images kept.
    #[error("{0} is not a media type of a blob")]
    MediaType(String),
}

impl std::str::FromStr for BlobId {
    type Err = BlobRefused;

    fn from_str(hex: &str) -> Result<Self, Self::Err> {
        if hex.len() == 64 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            Ok(Self(hex.to_string()))
        } else {
            Err(BlobRefused::Id(hex.to_string()))
        }
    }
}

/// The media types of blobs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MediaType {
    /// `image/png`.
    Png,
    /// `image/jpeg`.
    Jpeg,
    /// `image/webp`.
    Webp,
    /// `image/gif`.
    Gif,
    /// `application/x-protobuf`: a module's message.
    Protobuf,
    /// `application/x-tar`: a dynamic module's bundle.
    Tar,
}

impl MediaType {
    /// Its name, as HTTP and the wire carry it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Webp => "image/webp",
            Self::Gif => "image/gif",
            Self::Protobuf => PROTOBUF,
            Self::Tar => TAR,
        }
    }
}

impl std::str::FromStr for MediaType {
    type Err = BlobRefused;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        match name {
            "image/png" => Ok(Self::Png),
            "image/jpeg" => Ok(Self::Jpeg),
            "image/webp" => Ok(Self::Webp),
            "image/gif" => Ok(Self::Gif),
            PROTOBUF => Ok(Self::Protobuf),
            TAR => Ok(Self::Tar),
            other => Err(BlobRefused::MediaType(other.to_string())),
        }
    }
}

/// The media type of a module's message.
const PROTOBUF: &str = "application/x-protobuf";
/// The media type of a dynamic module's bundle.
const TAR: &str = "application/x-tar";

/// What `bytes` are, as their putter declares them (`declared`): empty, an
/// image known by its first bytes; `application/x-protobuf`, a module's
/// message; `application/x-tar`, a dynamic module's bundle (no pixels).
///
/// # Errors
/// No bytes, over [`MAX_BLOB_BYTES`], not an image kept, or a declared
/// media type that is neither.
pub fn blob(bytes: &[u8], declared: &str) -> Result<Image, BlobRefused> {
    match declared {
        "" => image(bytes),
        PROTOBUF => declared_blob(bytes, MediaType::Protobuf, MAX_BLOB_BYTES),
        TAR => declared_blob(bytes, MediaType::Tar, MAX_BUNDLE_BYTES),
        other => Err(BlobRefused::MediaType(other.to_string())),
    }
}

/// `bytes` as what their putter declared, within `limit` (no pixels).
fn declared_blob(bytes: &[u8], media_type: MediaType, limit: usize) -> Result<Image, BlobRefused> {
    if bytes.is_empty() {
        return Err(BlobRefused::Empty);
    }
    if bytes.len() > limit {
        return Err(BlobRefused::TooLarge {
            size: bytes.len(),
            limit,
        });
    }
    Ok(Image {
        media_type,
        width: 0,
        height: 0,
    })
}

/// What a blob is: its media type and, for an image, its size in pixels
/// (0 for a message).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Image {
    /// Its media type.
    pub media_type: MediaType,
    /// Its width in pixels.
    pub width: u32,
    /// Its height in pixels.
    pub height: u32,
}

/// What `bytes` are, known by their first bytes.
///
/// # Errors
/// No bytes, over [`MAX_BLOB_BYTES`], or not an image kept.
pub fn image(bytes: &[u8]) -> Result<Image, BlobRefused> {
    if bytes.is_empty() {
        return Err(BlobRefused::Empty);
    }
    if bytes.len() > MAX_BLOB_BYTES {
        return Err(BlobRefused::TooLarge {
            size: bytes.len(),
            limit: MAX_BLOB_BYTES,
        });
    }
    let media_type = match imagesize::image_type(bytes) {
        Ok(imagesize::ImageType::Png) => MediaType::Png,
        Ok(imagesize::ImageType::Jpeg) => MediaType::Jpeg,
        Ok(imagesize::ImageType::Webp) => MediaType::Webp,
        Ok(imagesize::ImageType::Gif) => MediaType::Gif,
        _ => return Err(BlobRefused::NotAnImage),
    };
    let size = imagesize::blob_size(bytes).map_err(|_| BlobRefused::NotAnImage)?;
    let pixels = |n: usize| u32::try_from(n).map_err(|_| BlobRefused::NotAnImage);
    Ok(Image {
        media_type,
        width: pixels(size.width)?,
        height: pixels(size.height)?,
    })
}

/// A blob as a store keeps it: what it is, its size and when it was put.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlobInfo {
    /// Its id.
    pub id: BlobId,
    /// Its size in bytes.
    pub size: u64,
    /// What it is.
    pub image: Image,
    /// When it was first put.
    pub put_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2×3 PNG's first bytes: the signature and its header chunk.
    fn png() -> Vec<u8> {
        let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        bytes.extend_from_slice(&[0, 0, 0, 13]);
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&2u32.to_be_bytes());
        bytes.extend_from_slice(&3u32.to_be_bytes());
        bytes.extend_from_slice(&[8, 6, 0, 0, 0]);
        bytes
    }

    #[test]
    fn an_image_is_known_by_its_first_bytes() {
        assert_eq!(
            image(&png()),
            Ok(Image {
                media_type: MediaType::Png,
                width: 2,
                height: 3
            })
        );
        assert_eq!(
            image(b"GIF89a\x04\x00\x05\x00\x80\x00\x00\x00").map(|i| i.media_type),
            Ok(MediaType::Gif)
        );
        assert_eq!(
            image(b"%PDF-1.7 not an image"),
            Err(BlobRefused::NotAnImage)
        );
        assert_eq!(image(b""), Err(BlobRefused::Empty));
        let mut big = png();
        big.resize(MAX_BLOB_BYTES + 1, 0);
        assert_eq!(
            image(&big),
            Err(BlobRefused::TooLarge {
                size: MAX_BLOB_BYTES + 1,
                limit: MAX_BLOB_BYTES,
            })
        );
    }

    #[test]
    fn an_id_is_a_lowercase_sha256() {
        let id = BlobId::of_digest(&[0xab; 32]);
        assert_eq!(id.as_str().len(), 64);
        assert_eq!(id.as_str().parse::<BlobId>(), Ok(id.clone()));
        assert!("AB".repeat(32).parse::<BlobId>().is_err());
        assert!("ab".parse::<BlobId>().is_err());
        assert_eq!("image/webp".parse::<MediaType>(), Ok(MediaType::Webp));
        assert!("image/svg+xml".parse::<MediaType>().is_err());
    }

    #[test]
    fn a_message_is_kept_as_its_putter_declares_it_and_an_unknown_type_is_refused() {
        let kept = blob(b"\x08\x01", "application/x-protobuf").unwrap();
        assert_eq!(kept.media_type, MediaType::Protobuf);
        assert_eq!((kept.width, kept.height), (0, 0));
        assert_eq!(blob(&png(), "").unwrap().media_type, MediaType::Png);
        assert_eq!(blob(b"", "application/x-protobuf"), Err(BlobRefused::Empty));
        assert!(matches!(
            blob(b"x", "text/plain"),
            Err(BlobRefused::MediaType(_))
        ));
        assert_eq!("application/x-protobuf".parse(), Ok(MediaType::Protobuf));
    }
}
