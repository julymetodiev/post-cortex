// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What the Discord module needs of the world (docs/modules/discord.md):
//! Discord's REST API, its Gateway, and the bus its events go on. The
//! module's adapters fill them; tests fill them in memory.

use async_trait::async_trait;
use pcx_module_api::ModuleError;
use post_cortex_proto::discord::{CreateMessage, Guild, GuildChannel, Member, Role, User};

use crate::domain::gateway::{Frame, Sent};
use crate::domain::heard::Heard;

/// Discord's REST API, as the bot.
#[async_trait]
pub trait Rest: Send + Sync {
    /// The Gateway's WebSocket URL.
    ///
    /// # Errors
    /// The API refused or could not be reached.
    async fn gateway(&self) -> Result<String, ModuleError>;

    /// The bot's user.
    ///
    /// # Errors
    /// As [`Rest::gateway`].
    async fn me(&self) -> Result<User, ModuleError>;

    /// Member `user` of server `guild`.
    ///
    /// # Errors
    /// As [`Rest::gateway`].
    async fn member(&self, guild: &str, user: &str) -> Result<Member, ModuleError>;

    /// Server `guild`: who owns it.
    ///
    /// # Errors
    /// The API refused or failed.
    async fn guild(&self, guild: &str) -> Result<Guild, ModuleError>;

    /// The roles of server `guild`.
    ///
    /// # Errors
    /// As [`Rest::gateway`].
    async fn roles(&self, guild: &str) -> Result<Vec<Role>, ModuleError>;

    /// The channels of server `guild` (no threads).
    ///
    /// # Errors
    /// As [`Rest::gateway`].
    async fn channels(&self, guild: &str) -> Result<Vec<GuildChannel>, ModuleError>;

    /// Channel or thread `id`.
    ///
    /// # Errors
    /// As [`Rest::gateway`]; `NotFound` for none.
    async fn channel(&self, id: &str) -> Result<GuildChannel, ModuleError>;

    /// Send `message` to channel or thread `channel`: its id.
    ///
    /// # Errors
    /// As [`Rest::gateway`].
    async fn send(&self, channel: &str, message: &CreateMessage) -> Result<String, ModuleError>;

    /// Give member `member` of server `guild` role `role`.
    ///
    /// # Errors
    /// As [`Rest::gateway`].
    async fn give_role(&self, guild: &str, member: &str, role: &str) -> Result<(), ModuleError>;

    /// Set the tags of forum post `thread` to `tags`, by id.
    ///
    /// # Errors
    /// As [`Rest::gateway`].
    async fn retag(&self, thread: &str, tags: &[String]) -> Result<(), ModuleError>;

    /// Give member `member` of server `guild` a timeout until `until`
    /// (ISO 8601); none: lift theirs.
    ///
    /// # Errors
    /// As [`Rest::gateway`].
    async fn time_out(
        &self,
        guild: &str,
        member: &str,
        until: Option<&str>,
    ) -> Result<(), ModuleError>;

    /// Lock and archive thread `thread`.
    ///
    /// # Errors
    /// As [`Rest::gateway`].
    async fn lock(&self, thread: &str) -> Result<(), ModuleError>;

    /// Ban member `member` from server `guild`, deleting what they wrote in
    /// the `delete_seconds` before.
    ///
    /// # Errors
    /// As [`Rest::gateway`].
    async fn ban(&self, guild: &str, member: &str, delete_seconds: u32) -> Result<(), ModuleError>;
}

/// Discord's Gateway.
#[async_trait]
pub trait Gateway: Send + Sync {
    /// A connection to the Gateway at `url`.
    ///
    /// # Errors
    /// It could not be reached.
    async fn connect(&self, url: &str) -> Result<Box<dyn Link>, ModuleError>;
}

/// A connection to the Gateway.
#[async_trait]
pub trait Link: Send {
    /// The next frame; none once the Gateway closed the connection.
    ///
    /// # Errors
    /// The connection broke, or a frame is not the Gateway's.
    async fn next(&mut self) -> Result<Option<Frame>, ModuleError>;

    /// Send `sent`.
    ///
    /// # Errors
    /// The connection broke.
    async fn send(&mut self, sent: &Sent) -> Result<(), ModuleError>;
}

/// The bus, for what the module hears.
#[async_trait]
pub trait Events: Send + Sync {
    /// Publish `heard`. A bus that does not keep it is logged: what is
    /// heard next is still published.
    async fn publish(&self, heard: Heard);
}
