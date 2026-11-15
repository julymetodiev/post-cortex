// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The person a request is for (`x-pcx-person`, set by a gateway from the
//! person's session or personal key; docs/host.md): whose user card a
//! request reads and writes (docs/memory.md, cards).

use pcx_module_api::metadata;
use uuid::Uuid;

/// The person of an HTTP request (REST, an MCP request's parts); none: no
/// person, as on a local host.
///
/// # Errors
/// `400` for a value that is no user id.
pub fn of_headers(headers: &http::HeaderMap) -> Result<Option<Uuid>, (http::StatusCode, String)> {
    post_cortex_core::core::card::person(
        headers
            .get(metadata::PERSON)
            .and_then(|value| value.to_str().ok()),
    )
    .map_err(|e| (http::StatusCode::BAD_REQUEST, e.to_string()))
}

/// The person of a gRPC call; none: no person.
///
/// # Errors
/// `InvalidArgument` for a value that is no user id.
pub fn of_metadata(map: &tonic::metadata::MetadataMap) -> Result<Option<Uuid>, tonic::Status> {
    post_cortex_core::core::card::person(
        map.get(metadata::PERSON)
            .and_then(|value| value.to_str().ok()),
    )
    .map_err(|e| tonic::Status::invalid_argument(e.to_string()))
}
