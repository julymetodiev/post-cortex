// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What a module may do (T11): the capabilities the configuration grants a
//! third-party module, and what a module without conversation access gets
//! of an event.

use std::path::PathBuf;

use pcx_module_api::sandbox::{Policy, Root};
use pcx_module_api::{Capability, Event, Manifest};
use serde::{Deserialize, Serialize};

/// A capability as the configuration names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Grant {
    /// Read the conversation (prompts, answers, tool calls).
    ReadConversation,
    /// Write memory.
    WriteMemory,
    /// Open network connections.
    Network,
    /// Use the paths of its filesystem grants.
    Filesystem,
    /// Start processes.
    Spawn,
    /// Serve HTML views shown in a sandboxed frame.
    UiHtml,
    /// Ask the host's models.
    Models,
}

impl From<Grant> for Capability {
    fn from(grant: Grant) -> Self {
        match grant {
            Grant::ReadConversation => Self::ReadConversation,
            Grant::WriteMemory => Self::WriteMemory,
            Grant::Network => Self::Network,
            Grant::Filesystem => Self::Filesystem,
            Grant::Spawn => Self::Spawn,
            Grant::UiHtml => Self::UiHtml,
            Grant::Models => Self::Models,
        }
    }
}

/// A directory the configuration lets a module use (`filesystem` of its
/// entry in `daemon.toml`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootGrant {
    /// The directory, absolute.
    pub path: PathBuf,
    /// Whether the module may write under it.
    #[serde(default)]
    pub write: bool,
}

/// What a module's sandbox lets it use, as the person at the machine
/// grants it (`GrantModule`, docs/host.md, settings): its other grants
/// stay as they are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleGrants {
    /// The directories it may use, the first the default of a relative
    /// path; none: no filesystem grant.
    pub roots: Vec<RootGrant>,
    /// Whether its processes may use the network.
    pub network: bool,
    /// Whether it may start processes.
    pub spawn: bool,
}

/// The sandbox policy of a module granted `grants` and `roots`: the roots
/// with the filesystem grant, the network and new processes with theirs,
/// and the names under them it never reaches (`deny`, the daemon's).
pub fn sandbox_policy(grants: &[Grant], roots: &[RootGrant], deny: &[String]) -> Policy {
    let roots = if grants.contains(&Grant::Filesystem) {
        roots
            .iter()
            .map(|r| Root {
                path: r.path.clone(),
                write: r.write,
            })
            .collect()
    } else {
        Vec::new()
    };
    Policy {
        roots,
        network: grants.contains(&Grant::Network),
        spawn: grants.contains(&Grant::Spawn),
        deny: deny.to_vec(),
        local_only: false,
    }
}

/// The wire values of `grants`.
pub fn capabilities(grants: &[Grant]) -> Vec<i32> {
    grants.iter().map(|g| Capability::from(*g).into()).collect()
}

/// The first capability `manifest` asks for that `grants` does not give
/// (an unknown value counts as not given).
pub fn missing(manifest: &Manifest, grants: &[Grant]) -> Option<String> {
    let granted = capabilities(grants);
    manifest
        .capabilities
        .iter()
        .find(|c| !granted.contains(c))
        .map(|c| match Capability::try_from(*c) {
            Ok(capability) => capability.as_str_name().to_string(),
            Err(_) => format!("unknown capability {c}"),
        })
}

/// Whether `manifest` may read the conversation.
pub fn reads_conversation(manifest: &Manifest) -> bool {
    manifest
        .capabilities
        .contains(&Capability::ReadConversation.into())
}

/// `event` without what the conversation says: the prompt, the tool's input
/// and result, the answer, the transcript and the payload. What happened,
/// to which tool, where and when stays.
pub fn without_conversation(event: &Event) -> Event {
    Event {
        prompt: String::new(),
        tool_input: None,
        tool_response: None,
        last_assistant_message: String::new(),
        transcript_path: String::new(),
        payload: None,
        ..event.clone()
    }
}
