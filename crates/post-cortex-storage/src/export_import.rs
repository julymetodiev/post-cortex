// Copyright (c) 2025 Julius ML
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

//! The `ArchiveCodec` adapter: the `pcx export` file format.
//!
//! - [`mod@format`]: the version written ([`EXPORT_FORMAT_VERSION`]) and the
//!   one version gate, [`check_format_version`];
//! - [`codec`]: JSON and compression (detected from the bytes), written
//!   and read one part at a time, and [`JsonArchive`], the codec;
//! - `merge`: where restored messages go in a stream (both adapters).
//!
//! The archive's types are in `post_cortex_core::archive`; what goes into
//! an archive and how an import merges is the Transfer capability's.

pub mod codec;
pub mod format;
mod merge;

pub use codec::{ArchiveError, JsonArchive};
pub use format::{EXPORT_FORMAT_VERSION, EXPORT_SCHEMA_URL, check_format_version};
pub(crate) use merge::place_restored_messages;

#[cfg(test)]
mod merge_tests;
