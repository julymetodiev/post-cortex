// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What the bot does to members and posts, only as the settings' moderation
//! lets it: a timeout given or lifted, a post locked, a member banned. It
//! decides none of them: a workflow asks (docs/modules/discord.md).

use pcx_module_api::ModuleError;

use crate::domain::{self, moderation};
use crate::service::{Service, refused};

impl Service<'_> {
    /// Give member `member_id` a timeout of `minutes`, or lift theirs with
    /// 0: when it ends, none when lifted.
    ///
    /// # Errors
    /// `Denied` when the settings let the bot moderate nothing;
    /// `InvalidArgument` for what is no id or more than the settings'
    /// longest; `FailedPrecondition` for a member Discord lets the bot not
    /// moderate ([`moderation::reaches`]); the API failed.
    pub async fn time_out(
        &self,
        member_id: &str,
        minutes: u32,
    ) -> Result<Option<String>, ModuleError> {
        let member = domain::id("member_id", member_id).map_err(refused)?;
        let until = moderation::until(self.config, chrono::Utc::now(), minutes).map_err(refused)?;
        self.reached(member).await?;
        self.rest
            .time_out(&self.config.guild_id, member, until.as_deref())
            .await?;
        Ok(until)
    }

    /// Whether the bot may moderate `member`, read from the server now (its
    /// owner, the bot's and the member's roles), before Discord is asked:
    /// Discord answers a member it may not moderate only `Missing
    /// Permissions`.
    ///
    /// # Errors
    /// `FailedPrecondition` naming why not; the API failed.
    async fn reached(&self, member: &str) -> Result<(), ModuleError> {
        let guild = &self.config.guild_id;
        let owner = self.rest.guild(guild).await?.owner_id;
        let bot = self.rest.member(guild, &self.server.me).await?;
        let theirs = self.rest.member(guild, member).await?;
        let roles = self.rest.roles(guild).await?;
        moderation::reaches(&self.server.me, &owner, &bot, member, &theirs, &roles).map_err(refused)
    }

    /// Lock and archive post or thread `thread_id`.
    ///
    /// # Errors
    /// `Denied` when the settings let the bot moderate nothing, or for a
    /// thread of a channel it does not post to; the API failed.
    pub async fn lock_thread(&self, thread_id: &str) -> Result<(), ModuleError> {
        moderation::allowed(self.config).map_err(refused)?;
        let thread = self.ours(thread_id).await?;
        self.rest.lock(&thread.id).await?;
        self.kept.threads.lock().remove(&thread.id);
        Ok(())
    }

    /// Ban member `member_id`, deleting what they wrote in the
    /// `delete_message_hours` before.
    ///
    /// # Errors
    /// `Denied` when the settings let the bot ban no one;
    /// `InvalidArgument` for what is no id or too many hours;
    /// `FailedPrecondition` for a member Discord lets the bot not moderate;
    /// the API failed.
    pub async fn ban(&self, member_id: &str, delete_message_hours: u32) -> Result<(), ModuleError> {
        let member = domain::id("member_id", member_id).map_err(refused)?;
        let seconds = moderation::ban(self.config, delete_message_hours).map_err(refused)?;
        self.reached(member).await?;
        self.rest.ban(&self.config.guild_id, member, seconds).await
    }
}
