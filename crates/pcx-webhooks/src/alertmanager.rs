// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Alertmanager's webhook body (ARCHITECTURE section 8, the outside
//! world's JSON): decoded into its mirror message by the canonical proto3
//! JSON mapping, unknown fields skipped; past here everything is typed.

use post_cortex_proto::webhooks::AlertmanagerWebhook;

use crate::domain::Refused;

/// The webhook in `body`.
///
/// # Errors
/// A body that is not Alertmanager's JSON.
pub fn decode(body: &[u8]) -> Result<AlertmanagerWebhook, Refused> {
    serde_json::from_slice(body).map_err(|e| Refused::Body(e.to_string()))
}

#[cfg(test)]
pub mod tests;
