// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! The http module's ports filled: the system clock, the system's
//! resolver, the client over reqwest (`client`), and the built-in presets,
//! a file of the crate read by its descriptors.

mod client;

use std::net::SocketAddr;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use pcx_module_api::{ModuleError, ModuleErrorKind};
use post_cortex_proto::http::{BuiltIn, Preset};
use prost_reflect::{DescriptorPool, DynamicMessage};

use crate::domain::preset;
use crate::ports::{Clock, Resolver};

pub use client::Reqwest;

/// The built-in presets (docs/modules/http.md, presets): the JSON form of
/// a `pcx.http.v1.BuiltIn`.
const BUILT_IN: &str = include_str!("presets.json");

/// The system's clock.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// The system's resolver.
pub struct SystemResolver;

#[async_trait]
impl Resolver for SystemResolver {
    async fn resolve(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>, ModuleError> {
        tokio::net::lookup_host((host, port))
            .await
            .map(Iterator::collect)
            .map_err(|e| {
                ModuleError::new(
                    ModuleErrorKind::Unavailable,
                    format!("{host} does not resolve: {e}"),
                )
            })
    }
}

/// The built-in presets, each checked, no name twice.
///
/// # Errors
/// `Internal`: the file does not read as a `BuiltIn`, or a preset in it
/// breaks a rule (a defect of the crate, which its test finds).
pub fn built_in() -> Result<Vec<Preset>, ModuleError> {
    let defect = |what: String| {
        ModuleError::new(
            ModuleErrorKind::Internal,
            format!("the built-in presets: {what}"),
        )
    };
    let pool = DescriptorPool::decode(post_cortex_proto::HTTP_DESCRIPTOR_SET)
        .map_err(|e| defect(e.to_string()))?;
    let message = pool
        .get_message_by_name("pcx.http.v1.BuiltIn")
        .ok_or_else(|| defect("pcx.http.v1.BuiltIn is not in the descriptors".to_string()))?;
    let mut json = serde_json::Deserializer::from_str(BUILT_IN);
    let read =
        DynamicMessage::deserialize(message, &mut json).map_err(|e| defect(e.to_string()))?;
    let presets = read
        .transcode_to::<BuiltIn>()
        .map_err(|e| defect(e.to_string()))?
        .presets;
    let mut names: Vec<String> = Vec::with_capacity(presets.len());
    for p in &presets {
        preset::check(p, &names).map_err(|rule| defect(format!("{}: {rule}", p.name)))?;
        names.push(p.name.clone());
    }
    Ok(presets)
}

#[cfg(test)]
mod tests;
