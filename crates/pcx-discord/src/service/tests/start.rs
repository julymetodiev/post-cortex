// Copyright (c) 2026 Julius ML
// Licensed under the MIT License. See LICENSE at the workspace root.

//! What the module's start reads of its server, and how it refuses.

use post_cortex_proto::discord::ServerStatus;

use super::*;

#[tokio::test]
async fn a_bot_without_the_rights_its_settings_need_fails_the_start_saying_them() {
    let discord = Discord {
        powerless: true,
        ..Discord::default()
    };
    let refused = server(&config(), &discord).await.unwrap_err();
    assert_eq!(refused.kind, ModuleErrorKind::FailedPrecondition);
    assert!(
        refused
            .message
            .contains("in channel help the bot lacks: View Channel")
    );
    assert!(refused.message.contains("the bot lacks: Manage Roles"));
    // The link adds it with the nine rights it may need, never Administrator.
    let invite =
        "https://discord.com/oauth2/authorize?client_id=9&scope=bot&permissions=1391837924356";
    assert!(refused.message.contains(invite), "{}", refused.message);

    let elsewhere = DiscordConfig {
        guild_id: "77".to_string(),
        ..config()
    };
    let absent = server(&elsewhere, &discord).await.unwrap_err();
    assert_eq!(absent.kind, ModuleErrorKind::FailedPrecondition);
    assert_eq!(
        absent.message,
        format!("the bot is not in server 77: add it with {invite}")
    );

    let wrong = checked(DiscordConfig {
        guild_id: "PCX".to_string(),
        ..config()
    })
    .unwrap_err();
    assert_eq!(wrong.kind, ModuleErrorKind::InvalidArgument);
    assert_eq!(wrong.field.as_deref(), Some("guild_id"));
}

#[tokio::test]
async fn the_server_is_read_after_the_start_and_until_then_the_module_says_why_not() {
    let linked = Linked::default();
    let waiting = linked.status();
    assert!(!waiting.connected);
    assert_eq!(waiting.problem, "discord has not read its server yet");
    assert_eq!(
        linked.server().unwrap_err().kind,
        ModuleErrorKind::Unavailable
    );

    // What the bot lacks is kept as why, and read again the next time.
    let powerless = Discord {
        powerless: true,
        ..Discord::default()
    };
    let refused = linked.read(&config(), &powerless).await.unwrap_err();
    assert_eq!(refused.kind, ModuleErrorKind::FailedPrecondition);
    let lacking = linked.status();
    assert!(!lacking.connected);
    assert!(
        lacking.problem.contains("the bot lacks: Manage Roles")
            && lacking
                .problem
                .contains("https://discord.com/oauth2/authorize"),
        "{}",
        lacking.problem
    );

    let discord = Discord::default();
    let read = linked.read(&config(), &discord).await.unwrap();
    assert_eq!(read.me, "9");
    assert_eq!(
        linked.status(),
        ServerStatus {
            connected: true,
            problem: String::new()
        }
    );
    // Once read it is kept: the bot's rights are not asked of Discord again.
    let kept = linked.read(&config(), &powerless).await.unwrap();
    assert!(Arc::ptr_eq(&read, &kept));
}
