// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! A timeout, a lock and a ban: only as the settings' moderation lets the
//! bot, and only for the server's members and the posts it posts to.

use post_cortex_proto::discord::Moderation;

use super::*;

fn moderating(ban: bool) -> DiscordConfig {
    DiscordConfig {
        moderation: Some(Moderation {
            longest_timeout_minutes: 1440,
            ban,
        }),
        ..config()
    }
}

#[tokio::test]
async fn without_moderation_the_bot_times_out_locks_and_bans_no_one() {
    let world = World::new().await;
    world.discord.threads.lock().push(thread("300", "200"));
    let service = world.service();
    for refused in [
        service.time_out("7", 60).await.map(|_| ()),
        service.lock_thread("300").await,
        service.ban("7", 0).await,
    ] {
        assert_eq!(refused.unwrap_err().kind, ModuleErrorKind::Denied);
    }
    assert!(world.discord.timeouts.lock().is_empty());
    assert!(world.discord.locked.lock().is_empty());
    assert!(world.discord.banned.lock().is_empty());
}

#[tokio::test]
async fn a_timeout_is_given_within_the_longest_and_lifted_with_zero() {
    let world = World::with(moderating(false)).await;
    let service = world.service();
    let before = chrono::Utc::now();
    let until = service.time_out("7", 1440).await.unwrap().unwrap();
    let ends = chrono::DateTime::parse_from_rfc3339(&until).unwrap();
    let day = chrono::Duration::minutes(1440);
    assert!(ends >= before + day - chrono::Duration::seconds(1));
    assert!(ends <= chrono::Utc::now() + day);
    assert_eq!(service.time_out("7", 0).await.unwrap(), None);
    assert_eq!(
        *world.discord.timeouts.lock(),
        [("7".to_string(), Some(until)), ("7".to_string(), None)]
    );

    let longer = service.time_out("7", 1441).await.unwrap_err();
    assert_eq!(longer.kind, ModuleErrorKind::InvalidArgument);
    let no_id = service.time_out("ana", 60).await.unwrap_err();
    assert_eq!(no_id.kind, ModuleErrorKind::InvalidArgument);
    assert_eq!(world.discord.timeouts.lock().len(), 2);
}

#[tokio::test]
async fn a_lock_is_only_of_a_post_of_a_channel_it_posts_to() {
    let world = World::with(moderating(false)).await;
    world
        .discord
        .threads
        .lock()
        .extend([thread("300", "200"), thread("301", "201")]);
    let service = world.service();
    service.lock_thread("300").await.unwrap();
    // `general` it only hears.
    let refused = service.lock_thread("301").await.unwrap_err();
    assert_eq!(refused.kind, ModuleErrorKind::Denied);
    assert_eq!(*world.discord.locked.lock(), ["300"]);
}

#[tokio::test]
async fn a_ban_needs_the_settings_to_let_the_bot_ban() {
    let timing_out = World::with(moderating(false)).await;
    let refused = timing_out.service().ban("7", 1).await.unwrap_err();
    assert_eq!(refused.kind, ModuleErrorKind::Denied);
    assert!(timing_out.discord.banned.lock().is_empty());

    let banning = World::with(moderating(true)).await;
    let service = banning.service();
    service.ban("7", 1).await.unwrap();
    let too_much = service.ban("8", 169).await.unwrap_err();
    assert_eq!(too_much.kind, ModuleErrorKind::InvalidArgument);
    assert_eq!(*banning.discord.banned.lock(), [("7".to_string(), 3600)]);
}

#[tokio::test]
async fn the_owner_the_bot_and_a_member_not_below_it_are_not_moderated() {
    let world = World::with(moderating(true)).await;
    let service = world.service();
    for (member, why) in [
        ("1", "owns the server"),
        ("9", "does not moderate itself"),
        ("2", "the role Moderators"),
    ] {
        let refused = service.time_out(member, 60).await.unwrap_err();
        assert_eq!(
            refused.kind,
            ModuleErrorKind::FailedPrecondition,
            "{member}"
        );
        assert!(
            refused.message.contains(why),
            "{member}: {}",
            refused.message
        );
        let refused = service.ban(member, 1).await.unwrap_err();
        assert_eq!(
            refused.kind,
            ModuleErrorKind::FailedPrecondition,
            "{member}"
        );
    }
    assert!(
        world.discord.timeouts.lock().is_empty(),
        "Discord is not asked"
    );
    assert!(world.discord.banned.lock().is_empty());
}

#[tokio::test]
async fn moderation_asks_its_rights_of_the_bot_and_a_longest_timeout_discord_gives() {
    let powerless = Discord {
        powerless: true,
        ..Discord::default()
    };
    let refused = server(&moderating(true), &powerless).await.unwrap_err();
    assert!(
        refused
            .message
            .contains("the bot lacks: Manage Roles, Timeout Members, Ban Members"),
        "{}",
        refused.message
    );
    assert!(
        refused.message.contains("Manage Threads"),
        "{}",
        refused.message
    );

    for minutes in [0, 40321] {
        let mut wrong = moderating(false);
        if let Some(moderation) = wrong.moderation.as_mut() {
            moderation.longest_timeout_minutes = minutes;
        }
        let refused = checked(wrong).unwrap_err();
        assert_eq!(
            refused.field.as_deref(),
            Some("moderation.longest_timeout_minutes"),
            "{minutes}"
        );
    }
}
