// Copyright (c) 2025, 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! Protobuf + tonic-generated wire types for post-cortex.
//!
//! Single source of truth for the post-cortex gRPC schema. Downstream
//! gRPC clients can depend on this crate alone — they pull `prost`,
//! `prost-types`, and `tonic` (with its default transport features),
//! but **not** the post-cortex-daemon server runtime or rmcp.
//!
//! # Public surface
//!
//! The [`pb`] module re-exports everything generated from
//! `proto/pcx.proto` (package `pcx.v1`). For convenience the most
//! frequently used wire types are also re-exported at the crate root —
//! see the top-level `pub use pb::*` below.
//!
//! # Stability
//!
//! Breaking changes to the proto schema are breaking changes to this
//! crate. SemVer applies normally; consumers should pin a minor version
//! (`= "0.x"`) until 1.0.

#![forbid(unsafe_code)]
// Generated code from prost/tonic does not satisfy the strict doc lints;
// scope the deny to our own modules only.
#![deny(rustdoc::broken_intra_doc_links)]

/// The `pcx.module.v1` module protocol (ARCHITECTURE section 7): the
/// types every module and the host speak, with their proto3 JSON form
/// (the hook JSON of Claude Code and Codex).
pub mod module {
    tonic::include_proto!("pcx.module.v1");
    include!(concat!(env!("OUT_DIR"), "/pcx.module.v1.serde.rs"));
}

/// The `pcx.blocks.v1` pieces of a conversation's message (W25): what
/// memory's API keeps of a message and what a model's request and answer
/// carry in the module protocol, with their proto3 JSON form.
pub mod blocks {
    tonic::include_proto!("pcx.blocks.v1");
    include!(concat!(env!("OUT_DIR"), "/pcx.blocks.v1.serde.rs"));
}

/// The `pcx.view.v1` views a module declares (ARCHITECTURE section 7,
/// Module UI levels 2 and 3), with their proto3 JSON form.
pub mod ui {
    tonic::include_proto!("pcx.view.v1");
    include!(concat!(env!("OUT_DIR"), "/pcx.view.v1.serde.rs"));
}

/// The `pcx.agent.v1` service: a client runs the agent's turns (see
/// `proto/pcx_agent.proto`).
pub mod agent {
    tonic::include_proto!("pcx.agent.v1");
}

/// The `pcx.views.v1` service: the typed views an app shows over memory
/// (see `proto/views.proto`).
pub mod views {
    tonic::include_proto!("pcx.views.v1");
}

/// The chief's package (W23).
pub mod chief {
    include!(concat!(env!("OUT_DIR"), "/modules/chief/pcx.chief.v1.rs"));
}

/// The chief's descriptors, which its manifest carries.
pub const CHIEF_DESCRIPTOR_SET: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/chief_descriptor.bin"));

/// The skills' package (W23).
pub mod skills {
    include!(concat!(env!("OUT_DIR"), "/modules/skills/pcx.skills.v1.rs"));
}

/// The skills' descriptors, which their manifest carries.
pub const SKILLS_DESCRIPTOR_SET: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/skills_descriptor.bin"));

/// The team's package (W23).
pub mod team {
    include!(concat!(env!("OUT_DIR"), "/modules/team/pcx.team.v1.rs"));
}

/// The team's descriptors, which its manifest carries.
pub const TEAM_DESCRIPTOR_SET: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/team_descriptor.bin"));

/// The board's package (W23).
pub mod board {
    include!(concat!(env!("OUT_DIR"), "/modules/board/pcx.board.v1.rs"));
}

/// The board's descriptors (with the team's, whose documents it reads),
/// which its manifest carries.
pub const BOARD_DESCRIPTOR_SET: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/board_descriptor.bin"));

/// The package of `tools-fs` (W23).
pub mod tools_fs {
    include!(concat!(
        env!("OUT_DIR"),
        "/modules/tools_fs/pcx.tools.fs.v1.rs"
    ));
}

/// The descriptors of `tools-fs`, which its manifest carries.
pub const TOOLS_FS_DESCRIPTOR_SET: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/tools_fs_descriptor.bin"));

/// The package of `tools-git` (W23).
pub mod tools_git {
    include!(concat!(
        env!("OUT_DIR"),
        "/modules/tools_git/pcx.tools.git.v1.rs"
    ));
}

/// The descriptors of `tools-git`, which its manifest carries.
pub const TOOLS_GIT_DESCRIPTOR_SET: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/tools_git_descriptor.bin"));

/// The package of `tools-shell` (W23).
pub mod tools_shell {
    include!(concat!(
        env!("OUT_DIR"),
        "/modules/tools_shell/pcx.tools.shell.v1.rs"
    ));
}

/// The descriptors of `tools-shell`, which its manifest carries.
pub const TOOLS_SHELL_DESCRIPTOR_SET: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/tools_shell_descriptor.bin"));

/// The scheduler's package (W28).
pub mod scheduler {
    include!(concat!(
        env!("OUT_DIR"),
        "/modules/scheduler/pcx.scheduler.v1.rs"
    ));
}

/// The scheduler's descriptors, which its manifest carries.
pub const SCHEDULER_DESCRIPTOR_SET: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/scheduler_descriptor.bin"));

/// The channel service every channel module serves (W29).
pub mod channel {
    include!(concat!(
        env!("OUT_DIR"),
        "/modules/channel/pcx.channel.v1.rs"
    ));
}

/// Telegram's package (W29): its settings, and the Bot API mirrored, with
/// the proto3 JSON mapping its adapter reads and writes the API with.
pub mod telegram {
    include!(concat!(
        env!("OUT_DIR"),
        "/modules/telegram/pcx.telegram.v1.rs"
    ));
    include!(concat!(
        env!("OUT_DIR"),
        "/modules/telegram/pcx.telegram.v1.serde.rs"
    ));
}

/// Telegram's descriptors with the channel service's, which its manifest
/// carries.
pub const TELEGRAM_DESCRIPTOR_SET: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/telegram_descriptor.bin"));

/// Discord's package (docs/modules/discord.md): its service, settings and
/// events, and Discord's REST and Gateway JSON mirrored, with the proto3
/// JSON mapping its adapters read and write them with.
pub mod discord {
    include!(concat!(
        env!("OUT_DIR"),
        "/modules/discord/pcx.discord.v1.rs"
    ));
    include!(concat!(
        env!("OUT_DIR"),
        "/modules/discord/pcx.discord.v1.serde.rs"
    ));
}

/// Discord's descriptors with the channel service's, which its manifest
/// carries.
pub const DISCORD_DESCRIPTOR_SET: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/discord_descriptor.bin"));

/// Observe's package (W30), with the proto3 JSON mapping of its mirrors of
/// Prometheus, Loki, Alertmanager and Kubernetes answers.
pub mod observe {
    include!(concat!(
        env!("OUT_DIR"),
        "/modules/observe/pcx.observe.v1.rs"
    ));
    include!(concat!(
        env!("OUT_DIR"),
        "/modules/observe/pcx.observe.v1.serde.rs"
    ));
}

/// Observe's descriptors, which its manifest carries.
pub const OBSERVE_DESCRIPTOR_SET: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/observe_descriptor.bin"));

/// Ops' package (W31), with the proto3 JSON mapping of its mirrors of
/// Kubernetes patches and objects.
pub mod ops {
    include!(concat!(env!("OUT_DIR"), "/modules/ops/pcx.ops.v1.rs"));
    include!(concat!(env!("OUT_DIR"), "/modules/ops/pcx.ops.v1.serde.rs"));
}

/// Ops' descriptors, which its manifest carries.
pub const OPS_DESCRIPTOR_SET: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/ops_descriptor.bin"));

/// Mail's package (docs/modules/mail.md), with the proto3 JSON mapping of
/// its messages that mirror Resend's and Postmark's APIs.
pub mod mail {
    include!(concat!(env!("OUT_DIR"), "/modules/mail/pcx.mail.v1.rs"));
    include!(concat!(
        env!("OUT_DIR"),
        "/modules/mail/pcx.mail.v1.serde.rs"
    ));
}

/// Mail's descriptors, which its manifest carries.
pub const MAIL_DESCRIPTOR_SET: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/mail_descriptor.bin"));

/// The dynamic module's package (docs/modules/dynamic.md).
pub mod dynamic {
    include!(concat!(
        env!("OUT_DIR"),
        "/modules/dynamic/pcx.dynamic.v1.rs"
    ));
}

/// The dynamic module's descriptors, which its manifest carries.
pub const DYNAMIC_DESCRIPTOR_SET: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/dynamic_descriptor.bin"));

/// The http module's package (docs/modules/http.md).
pub mod http {
    include!(concat!(env!("OUT_DIR"), "/modules/http/pcx.http.v1.rs"));
}

/// The http module's descriptors, which its manifest carries.
pub const HTTP_DESCRIPTOR_SET: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/http_descriptor.bin"));

/// The workflow's package (docs/modules/workflow.md).
pub mod workflow {
    include!(concat!(
        env!("OUT_DIR"),
        "/modules/workflow/pcx.workflow.v1.rs"
    ));
}

/// The workflow's descriptors, which its manifest carries and its
/// expressions are checked with.
pub const WORKFLOW_DESCRIPTOR_SET: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/workflow_descriptor.bin"));

/// The webhooks' package (W27), with the proto3 JSON mapping of its
/// messages that mirror an outside API (Alertmanager's body).
pub mod webhooks {
    include!(concat!(
        env!("OUT_DIR"),
        "/modules/webhooks/pcx.webhooks.v1.rs"
    ));
    include!(concat!(
        env!("OUT_DIR"),
        "/modules/webhooks/pcx.webhooks.v1.serde.rs"
    ));
}

/// The webhooks' descriptors, which their manifest carries.
pub const WEBHOOKS_DESCRIPTOR_SET: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/webhooks_descriptor.bin"));

/// The models module's package: its settings, a section for each
/// provider (docs/modules/agent.md, the providers).
pub mod models {
    include!(concat!(env!("OUT_DIR"), "/modules/models/pcx.models.v1.rs"));
}

/// The models module's descriptors, which its manifest carries.
pub const MODELS_DESCRIPTOR_SET: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/models_descriptor.bin"));

/// The agent's descriptors (`pcx.agent.v1`, its settings), which its
/// manifest carries.
pub const AGENT_DESCRIPTOR_SET: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/agent_descriptor.bin"));

/// The `pcx.tunnel.v1` services: a daemon pairs with an organisation of
/// the cloud and holds its tunnel to the gateway (see
/// `proto/pcx_tunnel.proto`, `docs/connect.md`).
pub mod tunnel {
    tonic::include_proto!("pcx.tunnel.v1");
}

/// Tonic-generated bindings for the `pcx.v1` proto package.
///
/// Equivalent to `include!(concat!(env!("OUT_DIR"), "/pcx.v1.rs"))` —
/// see `proto/pcx.proto` for the source schema.
pub mod pb {
    tonic::include_proto!("pcx.v1");
}

/// The encoded `FileDescriptorSet` of `pcx.v1` (`proto/pcx.proto`): the
/// field names and numbers of every request and answer, so a gateway can
/// read the fields the operation table names
/// (`post_cortex_memory::tools::operations`) from the wire.
pub const PCX_DESCRIPTOR_SET: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/pcx_descriptor.bin"));

/// The encoded `FileDescriptorSet` of `pcx.views.v1` (`proto/views.proto`,
/// with the well-known types it imports), for the same purpose.
pub const VIEWS_DESCRIPTOR_SET: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/views_descriptor.bin"));

// Top-level re-exports of the most commonly-used wire types so the rest of
// the workspace can write `post_cortex_proto::FreshnessEntry` instead of
// `post_cortex_proto::pb::FreshnessEntry`. Matches the names that used to
// live under `crate::daemon::grpc_service::pb` in the legacy single-crate
// layout — this is the migration that unblocks the Phase 3 core split.
pub use pb::{CascadeInvalidateReport, FreshnessEntry, FreshnessStatus, SourceReference, SymbolId};

/// HTTP header naming the caller as the author of its writes, for clients
/// whose MCP `clientInfo` does not say who they are (the public APIs of the
/// daemon and the cloud read it).
pub const AUTHOR_HEADER: &str = "x-pcx-author";
